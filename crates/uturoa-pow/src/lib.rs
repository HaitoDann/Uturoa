//! RandomX pour la frappe (SPEC §5).
//!
//! [`RandomXHasher`] implémente [`PowHasher`] en mode léger (256 Mo de
//! mémoire) : c'est ce qu'utilisent les validateurs pour vérifier les parts.
//! La clé RandomX change tous les `SEED_PERIOD` blocs ; la machine virtuelle
//! est reconstruite à chaque changement de clé (environ une demi-seconde).

use randomx_rs::{RandomXCache, RandomXFlag, RandomXVM};
use std::sync::Mutex;
use uturoa_core::{Hash, MintShare};
use uturoa_state::{PowHasher, meets_target};

/// Empreinte renvoyée si RandomX échoue : elle ne satisfait aucune
/// difficulté supérieure à 1, donc la part est refusée plutôt qu'acceptée à tort.
const FAILURE: Hash = [0xFF; 32];

pub struct RandomXHasher {
    flags: RandomXFlag,
    vm: Mutex<Option<(Hash, RandomXVM)>>,
}

impl Default for RandomXHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomXHasher {
    pub fn new() -> Self {
        Self {
            flags: RandomXFlag::get_recommended_flags(),
            vm: Mutex::new(None),
        }
    }

    fn compute(&self, seed: &Hash, input: &[u8]) -> Option<Hash> {
        let mut guard = self.vm.lock().ok()?;
        if guard.as_ref().is_none_or(|(s, _)| s != seed) {
            let cache = RandomXCache::new(self.flags, seed).ok()?;
            let vm = RandomXVM::new(self.flags, Some(cache), None).ok()?;
            *guard = Some((*seed, vm));
        }
        let (_, vm) = guard.as_ref()?;
        let out = vm.calculate_hash(input).ok()?;
        out.try_into().ok()
    }
}

impl PowHasher for RandomXHasher {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        self.compute(seed, input).unwrap_or(FAILURE)
    }
}

/// Cherche, en au plus `max_tries` essais, un nonce qui rend la part valide
/// pour `difficulty`, en partant du nonce de `template`.
pub fn mine(
    hasher: &dyn PowHasher,
    seed: &Hash,
    template: &MintShare,
    difficulty: u64,
    max_tries: u64,
) -> Option<MintShare> {
    let mut share = template.clone();
    for _ in 0..max_tries {
        if meets_target(&hasher.hash(seed, &share.pow_input()), difficulty) {
            return Some(share);
        }
        share.nonce = share.nonce.wrapping_add(1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use uturoa_core::Address;

    fn template() -> MintShare {
        MintShare {
            chain_id: "utu-test".into(),
            beneficiary: Address::from_bytes([1; 20]),
            anchor_height: 1,
            anchor_hash: [2; 32],
            nonce: 0,
        }
    }

    #[test]
    fn randomx_is_deterministic_and_seed_dependent() {
        let h = RandomXHasher::new();
        let input = template().pow_input();
        let a = h.hash(&[1; 32], &input);
        assert_ne!(a, FAILURE);
        assert_eq!(h.hash(&[1; 32], &input), a);
        assert_ne!(h.hash(&[2; 32], &input), a);
        // Retour à la première clé : même résultat.
        assert_eq!(h.hash(&[1; 32], &input), a);
        // Une seconde instance donne la même empreinte.
        assert_eq!(RandomXHasher::new().hash(&[1; 32], &input), a);
    }

    #[test]
    fn mining_finds_a_valid_share() {
        let h = RandomXHasher::new();
        let seed = [7; 32];
        let share = mine(&h, &seed, &template(), 4, 400).unwrap();
        assert!(meets_target(&h.hash(&seed, &share.pow_input()), 4));
        assert!(mine(&h, &seed, &template(), u64::MAX, 3).is_none());
    }
}
