//! Frappe de la phase 1 : vérification des parts, partage, difficulté (SPEC §5).

use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use uturoa_core::{Address, Amount, Hash, MintShare};

/// Ancienneté maximale du bloc d'ancrage d'une part, en blocs.
pub const MINT_ANCHOR_WINDOW: u64 = 64;

/// Période de partage de la frappe, en blocs (10 minutes à 12 s).
pub const MINT_PERIOD: u64 = 50;

/// Nombre de parts visé par période ; la difficulté s'ajuste pour l'atteindre.
pub const TARGET_SHARES_PER_PERIOD: u64 = 2_000;

/// Difficulté initiale de la frappe.
pub const INITIAL_MINT_DIFFICULTY: u64 = 1_000;

/// La clé RandomX change tous les `SEED_PERIOD` blocs…
pub const SEED_PERIOD: u64 = 2_048;

/// … avec un décalage de `SEED_LAG` blocs, pour que les mineurs la préparent.
pub const SEED_LAG: u64 = 64;

/// Calcul de l'empreinte de preuve de travail : RandomX en production
/// (crate `uturoa-pow`), une fonction quelconque dans les tests.
/// Doit être déterministe.
pub trait PowHasher {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash;
}

/// Raison du refus d'une part de frappe.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ShareError {
    #[error("la phase de frappe est terminée")]
    PhaseOver,
    #[error("trop de parts dans le bloc")]
    TooMany,
    #[error("identifiant de chaîne incorrect")]
    WrongChain,
    #[error("bloc d'ancrage trop ancien, futur ou inconnu")]
    BadAnchor,
    #[error("part déjà acceptée")]
    Duplicate,
    #[error("empreinte au-dessus de la cible")]
    InsufficientWork,
}

/// Une empreinte satisfait-elle la difficulté ? Ses 8 premiers octets, lus en
/// grand-boutiste, doivent être au plus `u64::MAX / difficulté`.
pub fn meets_target(hash: &Hash, difficulty: u64) -> bool {
    let mut head = [0u8; 8];
    for (h, b) in head.iter_mut().zip(hash.iter()) {
        *h = *b;
    }
    let value = u64::from_be_bytes(head);
    value <= u64::MAX.checked_div(difficulty.max(1)).unwrap_or(u64::MAX)
}

/// Hauteur du bloc dont l'empreinte sert de clé RandomX au bloc `height`.
pub fn seed_height(height: u64) -> u64 {
    height
        .saturating_sub(SEED_LAG)
        .checked_div(SEED_PERIOD)
        .unwrap_or(0)
        .saturating_mul(SEED_PERIOD)
}

/// Difficulté de la période suivante : proportionnelle au nombre de parts
/// reçues par rapport à la cible, entre un quart et quatre fois l'actuelle.
pub fn next_difficulty(current: u64, shares: u64) -> u64 {
    let cur = u128::from(current.max(1));
    let wanted = cur
        .saturating_mul(u128::from(shares))
        .checked_div(u128::from(TARGET_SHARES_PER_PERIOD))
        .unwrap_or(0);
    let low = cur.checked_div(4).unwrap_or(0).max(1);
    let high = cur.saturating_mul(4);
    u64::try_from(wanted.clamp(low, high)).unwrap_or(u64::MAX)
}

/// État de la frappe, inclus dans la racine d'état.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MintState {
    pub difficulty: u64,
    /// Empreintes des blocs récents et des blocs de clé, par hauteur.
    pub(crate) block_hashes: BTreeMap<u64, Hash>,
    /// Parts déjà acceptées, par hauteur d'ancrage.
    pub(crate) seen: BTreeMap<u64, BTreeSet<Hash>>,
    /// Parts de la période en cours, par bénéficiaire.
    pub(crate) period_shares: BTreeMap<Address, u64>,
    pub(crate) period_total: u64,
    /// Unités frappées, pas encore partagées.
    pub(crate) pot: Amount,
}

impl MintState {
    pub fn new(difficulty: u64) -> Self {
        Self {
            difficulty: difficulty.max(1),
            block_hashes: BTreeMap::new(),
            seen: BTreeMap::new(),
            period_shares: BTreeMap::new(),
            period_total: 0,
            pot: 0,
        }
    }

    pub fn pot(&self) -> Amount {
        self.pot
    }

    pub fn period_total(&self) -> u64 {
        self.period_total
    }

    /// Clé RandomX des parts incluses dans le bloc `height`.
    pub fn seed_for(&self, height: u64) -> Option<Hash> {
        self.block_hashes.get(&seed_height(height)).copied()
    }

    /// Enregistre l'empreinte du bloc `height - 1` et oublie ce qui ne sert plus.
    pub(crate) fn record_parent(&mut self, height: u64, parent_hash: Hash) {
        let parent = height.saturating_sub(1);
        self.block_hashes.insert(parent, parent_hash);
        let oldest_anchor = height.saturating_sub(MINT_ANCHOR_WINDOW);
        let oldest_seed = seed_height(height).saturating_sub(SEED_PERIOD);
        self.block_hashes.retain(|h, _| {
            *h >= oldest_anchor || (*h >= oldest_seed && h.checked_rem(SEED_PERIOD) == Some(0))
        });
        self.seen.retain(|h, _| *h >= oldest_anchor);
    }

    /// Vérifie et compte une part pour le bloc `height`.
    pub(crate) fn accept(
        &mut self,
        chain_id: &str,
        height: u64,
        share: &MintShare,
        pow: &dyn PowHasher,
    ) -> Result<(), ShareError> {
        if share.chain_id != chain_id {
            return Err(ShareError::WrongChain);
        }
        let oldest = height.saturating_sub(MINT_ANCHOR_WINDOW);
        if share.anchor_height >= height || share.anchor_height < oldest {
            return Err(ShareError::BadAnchor);
        }
        if self.block_hashes.get(&share.anchor_height) != Some(&share.anchor_hash) {
            return Err(ShareError::BadAnchor);
        }
        let id = share.id();
        if self
            .seen
            .get(&share.anchor_height)
            .is_some_and(|s| s.contains(&id))
        {
            return Err(ShareError::Duplicate);
        }
        let seed = self.seed_for(height).ok_or(ShareError::BadAnchor)?;
        if !meets_target(&pow.hash(&seed, &share.pow_input()), self.difficulty) {
            return Err(ShareError::InsufficientWork);
        }
        self.seen.entry(share.anchor_height).or_default().insert(id);
        let count = self.period_shares.entry(share.beneficiary).or_insert(0);
        *count = count.saturating_add(1);
        self.period_total = self.period_total.saturating_add(1);
        Ok(())
    }

    /// Partage le pot entre les parts de la période, au prorata, puis ajuste
    /// la difficulté. Renvoie les crédits ; le reste de la division reste au pot.
    pub(crate) fn close_period(&mut self) -> Vec<(Address, Amount)> {
        let mut credits = Vec::with_capacity(self.period_shares.len());
        if self.period_total > 0 {
            let total = u128::from(self.period_total);
            let pot = u128::from(self.pot);
            let mut paid: Amount = 0;
            for (address, count) in &self.period_shares {
                let share = pot
                    .saturating_mul(u128::from(*count))
                    .checked_div(total)
                    .unwrap_or(0);
                let share = Amount::try_from(share).unwrap_or(0);
                if share > 0 {
                    credits.push((*address, share));
                    paid = paid.saturating_add(share);
                }
            }
            self.pot = self.pot.saturating_sub(paid);
        }
        self.difficulty = next_difficulty(self.difficulty, self.period_total);
        self.period_shares.clear();
        self.period_total = 0;
        credits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_check() {
        assert!(meets_target(&[0xFF; 32], 1));
        assert!(!meets_target(&[0xFF; 32], 2));
        let mut h = [0u8; 32];
        h[0] = 0x7F;
        h[1..8].copy_from_slice(&[0xFF; 7]);
        assert!(meets_target(&h, 2));
        assert!(meets_target(&[0; 32], u64::MAX));
    }

    #[test]
    fn seed_changes_every_period_with_lag() {
        assert_eq!(seed_height(1), 0);
        assert_eq!(seed_height(SEED_PERIOD + SEED_LAG - 1), 0);
        assert_eq!(seed_height(SEED_PERIOD + SEED_LAG), SEED_PERIOD);
    }

    #[test]
    fn difficulty_tracks_target_within_bounds() {
        assert_eq!(next_difficulty(1_000, TARGET_SHARES_PER_PERIOD), 1_000);
        assert_eq!(next_difficulty(1_000, TARGET_SHARES_PER_PERIOD * 2), 2_000);
        assert_eq!(
            next_difficulty(1_000, TARGET_SHARES_PER_PERIOD * 100),
            4_000
        );
        assert_eq!(next_difficulty(1_000, 0), 250);
        assert_eq!(next_difficulty(1, 0), 1);
    }
}
