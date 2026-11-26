//! Validateurs, délégation, présence, sanctions, récompenses et bascule
//! (SPEC §4).
//!
//! Le moteur de consensus compte les votes à poids égal : chaque validateur
//! actif pèse `1 / n`. Le stake sert à **sélectionner** les validateurs (les
//! mieux classés) et à **partager** les récompenses. Avec au moins 10
//! validateurs actifs, aucun ne pèse plus de 10 % du vote.

use crate::state::{State, TxError};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::{BTreeMap, BTreeSet};
use uturoa_consensus::Evidence;
use uturoa_core::{
    Address, Amount, CommitCertificate, Hash, PublicKey, UNITS_PER_COIN, ValidatorInfo, decode,
};

/// Stake propre minimal d'un validateur : 10 000 pièces.
pub const MIN_VALIDATOR_STAKE: Amount = 10_000 * UNITS_PER_COIN;
/// Commission proposée par défaut par les portefeuilles : 10 %.
pub const DEFAULT_COMMISSION_BPS: u16 = 1_000;
/// Plafond absolu d'une commission : 100 %.
pub const COMMISSION_LIMIT_BPS: u16 = 10_000;
/// Hausse maximale de la commission par époque : 1 point.
pub const MAX_COMMISSION_RAISE_BPS: u16 = 100;
/// Part des délégations confisquée en cas de double signature : 1 %.
pub const DELEGATOR_SLASH_PERCENT: u128 = 1;
/// Époque : un jour de blocs de 12 s.
pub const EPOCH_BLOCKS: u64 = 7_200;
/// Présence minimale pour toucher sa part : 97 %.
pub const MIN_AVAILABILITY_BPS: u64 = 9_700;
/// Absences consécutives avant la mise à l'écart.
pub const ABSENCE_JAIL_BLOCKS: u64 = 1_000;
/// Durée minimale d'une mise à l'écart : une époque.
pub const JAIL_MIN_BLOCKS: u64 = EPOCH_BLOCKS;
/// Délai de restitution du stake : 21 jours.
pub const UNBONDING_BLOCKS: u64 = 21 * EPOCH_BLOCKS;
/// En dessous, on ne retire plus de validateur actif pour absence.
pub const MIN_ACTIVE_VALIDATORS: usize = 4;
/// Part de la caution confisquée versée à celui qui prouve la faute.
pub const REPORTER_REWARD_PERCENT: u128 = 10;

const MAX_NAME: usize = 64;
const MAX_SITE: usize = 128;
const MAX_CONTACT: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum ValidatorStatus {
    /// Peut être sélectionné.
    Candidate = 0,
    /// Mis à l'écart pour absence depuis la hauteur `since`.
    Jailed { since: u64 } = 1,
    /// A quitté l'ensemble ; son stake est en restitution.
    Left = 2,
    /// Exclu définitivement pour double signature.
    Banned = 3,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidatorRecord {
    pub key: PublicKey,
    pub info: ValidatorInfo,
    pub commission_bps: u16,
    /// Plafond déclaré à l'inscription, définitif.
    pub max_commission_bps: u16,
    /// Époque du dernier changement de commission.
    pub commission_epoch: u64,
    pub status: ValidatorStatus,
    /// Validateur d'amorçage de la phase 1.
    pub bootstrap: bool,
    /// Stake délégué par l'opérateur lui-même.
    pub self_stake: Amount,
    /// Stake délégué par les autres.
    pub delegated: Amount,
    pub joined_at: u64,
    /// Blocs consécutifs sans signature, quand il était attendu.
    pub missed_streak: u64,
    /// Signatures et blocs attendus sur l'époque en cours.
    pub epoch_signed: u64,
    pub epoch_expected: u64,
    /// Mis à l'écart pendant l'époque en cours.
    pub jailed_this_epoch: bool,
    /// Dernière version de protocole signalée.
    pub signaled_version: u32,
}

impl ValidatorRecord {
    pub fn total_stake(&self) -> Amount {
        self.self_stake.saturating_add(self.delegated)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Staking {
    pub(crate) validators: BTreeMap<Address, ValidatorRecord>,
    pub(crate) keys: BTreeMap<PublicKey, Address>,
    /// `(délégant, opérateur) → montant`.
    pub(crate) delegations: BTreeMap<(Address, Address), Amount>,
    /// `(hauteur de restitution, bénéficiaire) → montant`.
    pub(crate) unbonding: BTreeMap<(u64, Address), Amount>,
    /// Opérateurs actifs pour le prochain bloc, triés par adresse.
    pub(crate) active: Vec<Address>,
    /// Ensemble calculé pour le bloc d'après (délai de 2 blocs).
    pub(crate) staged: Option<Vec<Address>>,
    /// Ensembles de clés par hauteur de début, sur le délai de restitution.
    pub(crate) set_history: BTreeMap<u64, Vec<PublicKey>>,
    /// Preuves d'équivoque déjà sanctionnées.
    pub(crate) evidence_seen: BTreeSet<Hash>,
    /// Un retrait de validateur actif impose de recalculer l'ensemble.
    pub(crate) dirty: bool,
}

fn info_is_valid(info: &ValidatorInfo) -> bool {
    !info.name.is_empty()
        && info.name.len() <= MAX_NAME
        && info.site.len() <= MAX_SITE
        && info.contact.len() <= MAX_CONTACT
}

fn epoch_of(height: u64) -> u64 {
    height.checked_div(EPOCH_BLOCKS).unwrap_or(0)
}

/// `amount × num / den` en 128 bits, arrondi vers le bas.
fn pro_rata(amount: Amount, num: u128, den: u128) -> Amount {
    let v = u128::from(amount)
        .saturating_mul(num)
        .checked_div(den)
        .unwrap_or(0);
    Amount::try_from(v).unwrap_or(0)
}

impl Staking {
    /// Ensemble de clés en vigueur à la hauteur `height`.
    pub fn set_at(&self, height: u64) -> Vec<PublicKey> {
        self.set_history
            .range(..=height)
            .next_back()
            .map(|(_, keys)| keys.clone())
            .unwrap_or_default()
    }

    pub fn validator(&self, operator: &Address) -> Option<&ValidatorRecord> {
        self.validators.get(operator)
    }

    pub fn delegation(&self, delegator: &Address, operator: &Address) -> Amount {
        self.delegations
            .get(&(*delegator, *operator))
            .copied()
            .unwrap_or(0)
    }

    /// Opérateur d'une clé de consensus.
    pub fn operator_of(&self, key: &PublicKey) -> Option<Address> {
        self.keys.get(key).copied()
    }

    pub fn active(&self) -> &[Address] {
        &self.active
    }

    fn keys_of(&self, operators: &[Address]) -> Vec<PublicKey> {
        operators
            .iter()
            .filter_map(|o| self.validators.get(o).map(|v| v.key.clone()))
            .collect()
    }

    /// Ensemble actif pour la hauteur `target`.
    pub(crate) fn compute_set(&self, target: u64, phase_end: u64, max_active: u64) -> Vec<Address> {
        let bootstrap: Vec<Address> = self
            .validators
            .iter()
            .filter(|(_, v)| v.bootstrap && v.status == ValidatorStatus::Candidate)
            .map(|(a, _)| *a)
            .collect();
        if target <= phase_end {
            return bootstrap;
        }
        let mut ranked: Vec<(&Address, &ValidatorRecord)> = self
            .validators
            .iter()
            .filter(|(_, v)| {
                v.status == ValidatorStatus::Candidate && v.self_stake >= MIN_VALIDATOR_STAKE
            })
            .collect();
        ranked.sort_by(|a, b| b.1.total_stake().cmp(&a.1.total_stake()).then(a.0.cmp(b.0)));
        let max = usize::try_from(max_active).unwrap_or(usize::MAX);
        let mut chosen: Vec<Address> = ranked.into_iter().take(max).map(|(a, _)| *a).collect();
        if chosen.len() < MIN_ACTIVE_VALIDATORS {
            // Pas assez de validateurs à stake : l'amorçage continue.
            return bootstrap;
        }
        chosen.sort();
        chosen
    }

    /// Fait avancer l'ensemble à la fin du bloc `height` : celui calculé il y
    /// a un bloc entre en vigueur au bloc `height + 1`.
    pub(crate) fn rotate(&mut self, height: u64, phase_end: u64, max_active: u64) {
        let next = self.staged.take().unwrap_or_else(|| self.active.clone());
        if next != self.active {
            let keys = self.keys_of(&next);
            self.set_history.insert(height.saturating_add(1), keys);
            self.active = next;
        }
        let target = height.saturating_add(2);
        let switch = target == phase_end.saturating_add(1);
        let epoch_end = height.checked_rem(EPOCH_BLOCKS) == Some(0);
        if self.dirty || switch || epoch_end {
            let set = self.compute_set(target, phase_end, max_active);
            if set != self.active {
                self.staged = Some(set);
            }
            self.dirty = false;
        }
        // Historique limité au délai de restitution.
        let horizon = height.saturating_sub(UNBONDING_BLOCKS);
        while self.set_history.len() > 1 {
            let mut it = self.set_history.keys();
            let (Some(&first), Some(&second)) = (it.next(), it.next()) else {
                break;
            };
            if second <= horizon {
                self.set_history.remove(&first);
            } else {
                break;
            }
        }
    }

    /// Met en restitution toutes les délégations vers `operator`.
    fn unbond_all(&mut self, operator: &Address, release: u64) -> Result<(), TxError> {
        let entries: Vec<((Address, Address), Amount)> = self
            .delegations
            .iter()
            .filter(|((_, o), _)| o == operator)
            .map(|(k, v)| (*k, *v))
            .collect();
        for ((delegator, op), amount) in entries {
            self.delegations.remove(&(delegator, op));
            let slot = self.unbonding.entry((release, delegator)).or_insert(0);
            *slot = slot.checked_add(amount).ok_or(TxError::Overflow)?;
        }
        if let Some(v) = self.validators.get_mut(operator) {
            v.self_stake = 0;
            v.delegated = 0;
        }
        Ok(())
    }

    fn is_active(&self, operator: &Address) -> bool {
        self.active.binary_search(operator).is_ok()
    }
}

/// Effet d'une transaction de validation sur le solde de l'émetteur.
pub(crate) enum StakeDebit {
    None,
    Amount(Amount),
}

impl State {
    /// Validateurs d'amorçage de la genèse, à poids égal, actifs dès le bloc 1.
    pub(crate) fn add_bootstrap(&mut self, validators: Vec<(Address, PublicKey, ValidatorInfo)>) {
        for (operator, key, info) in validators {
            self.staking.keys.insert(key.clone(), operator);
            self.staking.validators.insert(
                operator,
                ValidatorRecord {
                    key,
                    info,
                    commission_bps: 0,
                    max_commission_bps: 0,
                    commission_epoch: 0,
                    status: ValidatorStatus::Candidate,
                    bootstrap: true,
                    self_stake: 0,
                    delegated: 0,
                    joined_at: 0,
                    missed_streak: 0,
                    epoch_signed: 0,
                    epoch_expected: 0,
                    jailed_this_epoch: false,
                    signaled_version: 0,
                },
            );
        }
        let set = self
            .staking
            .compute_set(1, self.phase_end, self.params.max_active_validators);
        let keys = self.staking.keys_of(&set);
        self.staking.active = set;
        self.staking.set_history.insert(1, keys);
    }

    /// Vérifie le certificat du bloc `height - 1` et compte la présence de
    /// chaque validateur **attendu**, c'est-à-dire actif à cette hauteur.
    pub(crate) fn record_commit(
        &mut self,
        height: u64,
        parent_hash: &Hash,
        commit: Option<&CommitCertificate>,
    ) -> Result<(), &'static str> {
        let prev = height.saturating_sub(1);
        let keys = self.staking.set_at(prev);
        if keys.is_empty() || prev == 0 {
            // Pas de validateurs (réseau d'essai) ou premier bloc : rien à vérifier.
            return Ok(());
        }
        let commit = commit.ok_or("certificat du bloc précédent manquant")?;
        if commit.height != prev || commit.block_hash != *parent_hash {
            return Err("certificat d'un autre bloc");
        }
        commit
            .verify(&self.chain_id, &keys)
            .map_err(|_| "certificat invalide")?;
        let signers: BTreeSet<u16> = commit.signatures.iter().map(|(i, _)| *i).collect();
        for (index, key) in keys.iter().enumerate() {
            let Some(operator) = self.staking.keys.get(key).copied() else {
                continue;
            };
            let signed = u16::try_from(index).is_ok_and(|i| signers.contains(&i));
            if let Some(v) = self.staking.validators.get_mut(&operator) {
                v.epoch_expected = v.epoch_expected.saturating_add(1);
                if signed {
                    v.epoch_signed = v.epoch_signed.saturating_add(1);
                    v.missed_streak = 0;
                } else {
                    v.missed_streak = v.missed_streak.saturating_add(1);
                }
            }
        }
        Ok(())
    }

    /// Met à l'écart les validateurs absents trop longtemps (phase 2 seulement).
    pub(crate) fn jail_absent(&mut self, height: u64) {
        if height <= self.phase_end {
            return;
        }
        let absent: Vec<Address> = self
            .staking
            .active
            .iter()
            .filter(|o| {
                self.staking.validators.get(o).is_some_and(|v| {
                    v.missed_streak >= ABSENCE_JAIL_BLOCKS && v.status == ValidatorStatus::Candidate
                })
            })
            .copied()
            .collect();
        let mut remaining = self.staking.active.len();
        for operator in absent {
            if remaining <= MIN_ACTIVE_VALIDATORS {
                break;
            }
            if let Some(v) = self.staking.validators.get_mut(&operator) {
                v.status = ValidatorStatus::Jailed { since: height };
                v.jailed_this_epoch = true;
                remaining = remaining.saturating_sub(1);
                self.staking.dirty = true;
            }
        }
    }

    /// Restitue le stake arrivé à échéance.
    pub(crate) fn release_unbonding(&mut self, height: u64) -> Result<(), &'static str> {
        let due: Vec<(u64, Address)> = self
            .staking
            .unbonding
            .range(..=(height, Address::from_bytes([0xFF; 20])))
            .map(|(k, _)| *k)
            .collect();
        for key in due {
            let amount = self.staking.unbonding.remove(&key).unwrap_or(0);
            let a = self.accounts.entry(key.1).or_default();
            a.balance = a.balance.checked_add(amount).ok_or("dépassement")?;
        }
        Ok(())
    }

    /// Fin d'époque en phase 2 : partage de la forge et signal de mise à jour.
    pub(crate) fn close_epoch(&mut self, height: u64) -> Result<(), &'static str> {
        let pot = self.pending_rewards;
        self.pending_rewards = 0;
        let eligible: Vec<Address> = if height > self.phase_end {
            self.staking
                .validators
                .iter()
                .filter(|(_, v)| {
                    v.status == ValidatorStatus::Candidate
                        && !v.jailed_this_epoch
                        && v.self_stake >= MIN_VALIDATOR_STAKE
                        && v.epoch_expected > 0
                        && u128::from(v.epoch_signed).saturating_mul(10_000)
                            >= u128::from(v.epoch_expected)
                                .saturating_mul(u128::from(MIN_AVAILABILITY_BPS))
                })
                .map(|(a, _)| *a)
                .collect()
        } else {
            Vec::new()
        };
        let total: u128 = eligible
            .iter()
            .filter_map(|a| self.staking.validators.get(a))
            .map(|v| u128::from(v.total_stake()))
            .sum();
        let mut paid: Amount = 0;
        if total > 0 {
            let mut by_operator: BTreeMap<Address, Vec<(Address, Amount)>> = BTreeMap::new();
            for ((delegator, operator), amount) in &self.staking.delegations {
                by_operator
                    .entry(*operator)
                    .or_default()
                    .push((*delegator, *amount));
            }
            let mut credits: Vec<(Address, Amount)> = Vec::new();
            for operator in &eligible {
                let Some(v) = self.staking.validators.get(operator) else {
                    continue;
                };
                let stake = v.total_stake();
                let share = pro_rata(pot, u128::from(stake), total);
                let commission = pro_rata(share, u128::from(v.commission_bps), 10_000);
                credits.push((*operator, commission));
                let rest = share.saturating_sub(commission);
                for (delegator, amount) in by_operator.get(operator).into_iter().flatten() {
                    credits.push((
                        *delegator,
                        pro_rata(rest, u128::from(*amount), u128::from(stake)),
                    ));
                }
            }
            for (address, amount) in credits {
                if amount == 0 {
                    continue;
                }
                let a = self.accounts.entry(address).or_default();
                a.balance = a.balance.checked_add(amount).ok_or("dépassement")?;
                paid = paid.checked_add(amount).ok_or("dépassement")?;
            }
        }
        // Ce qui n'est pas versé retourne au réservoir.
        let unpaid = pot.checked_sub(paid).ok_or("partage supérieur au pot")?;
        self.supply = self.supply.checked_sub(unpaid).ok_or("dépassement")?;

        // Mise à jour du protocole : plus des 2/3 des actifs prêts pour la suivante.
        let n = self.staking.active.len();
        let wanted = self.protocol_version.saturating_add(1);
        let ready = self
            .staking
            .active
            .iter()
            .filter(|o| {
                self.staking
                    .validators
                    .get(o)
                    .is_some_and(|v| v.signaled_version >= wanted)
            })
            .count();
        if n > 0 && ready.saturating_mul(3) > n.saturating_mul(2) {
            self.protocol_version = wanted;
        }

        for v in self.staking.validators.values_mut() {
            v.epoch_signed = 0;
            v.epoch_expected = 0;
            v.jailed_this_epoch = false;
        }
        Ok(())
    }

    /// Applique une transaction de validation pour `sender`, à la hauteur
    /// `height`. Les vérifications communes (autorisation, nonce, frais) sont
    /// déjà faites. Renvoie le montant à débiter en plus des frais.
    pub(crate) fn staking_tx(
        &mut self,
        sender: &Address,
        height: u64,
        body: &uturoa_core::TxBody,
    ) -> Result<StakeDebit, TxError> {
        use uturoa_core::TxBody;
        let release = height.saturating_add(UNBONDING_BLOCKS);
        match body {
            TxBody::RegisterValidator {
                key,
                info,
                commission_bps,
                max_commission_bps,
            } => {
                if self.staking.validators.contains_key(sender) {
                    return Err(TxError::Malformed("validateur déjà inscrit"));
                }
                if self.staking.keys.contains_key(key) {
                    return Err(TxError::Malformed("clé de consensus déjà utilisée"));
                }
                if *max_commission_bps > COMMISSION_LIMIT_BPS
                    || *commission_bps > *max_commission_bps
                    || !info_is_valid(info)
                {
                    return Err(TxError::Malformed("fiche ou commission invalide"));
                }
                self.staking.keys.insert(key.clone(), *sender);
                self.staking.validators.insert(
                    *sender,
                    ValidatorRecord {
                        key: key.clone(),
                        info: info.clone(),
                        commission_bps: *commission_bps,
                        max_commission_bps: *max_commission_bps,
                        commission_epoch: epoch_of(height),
                        status: ValidatorStatus::Candidate,
                        bootstrap: false,
                        self_stake: 0,
                        delegated: 0,
                        joined_at: height,
                        missed_streak: 0,
                        epoch_signed: 0,
                        epoch_expected: 0,
                        jailed_this_epoch: false,
                        signaled_version: 0,
                    },
                );
                Ok(StakeDebit::None)
            }
            TxBody::UpdateValidatorInfo {
                info,
                commission_bps,
            } => {
                let epoch = epoch_of(height);
                let v = self
                    .staking
                    .validators
                    .get_mut(sender)
                    .ok_or(TxError::Malformed("pas de validateur à ce nom"))?;
                if v.status == ValidatorStatus::Banned || !info_is_valid(info) {
                    return Err(TxError::Malformed("fiche invalide ou validateur exclu"));
                }
                if *commission_bps > v.max_commission_bps {
                    return Err(TxError::Malformed("commission au-delà du plafond déclaré"));
                }
                if *commission_bps > v.commission_bps {
                    // Hausse : 1 point au plus, une fois par époque.
                    let raise = commission_bps.saturating_sub(v.commission_bps);
                    if raise > MAX_COMMISSION_RAISE_BPS || v.commission_epoch == epoch {
                        return Err(TxError::Malformed(
                            "commission : hausse d'1 point par jour au plus",
                        ));
                    }
                }
                if *commission_bps != v.commission_bps {
                    // Une baisse est toujours permise ; elle compte comme le
                    // changement du jour.
                    v.commission_bps = *commission_bps;
                    v.commission_epoch = epoch;
                }
                v.info = info.clone();
                Ok(StakeDebit::None)
            }
            TxBody::Delegate { validator, amount } => {
                if *amount == 0 {
                    return Err(TxError::Malformed("montant nul"));
                }
                let operator = *self
                    .staking
                    .keys
                    .get(validator)
                    .ok_or(TxError::Malformed("validateur inconnu"))?;
                let v = self
                    .staking
                    .validators
                    .get_mut(&operator)
                    .ok_or(TxError::Malformed("validateur inconnu"))?;
                if matches!(v.status, ValidatorStatus::Left | ValidatorStatus::Banned) {
                    return Err(TxError::Malformed("validateur parti ou exclu"));
                }
                if operator == *sender {
                    v.self_stake = v.self_stake.checked_add(*amount).ok_or(TxError::Overflow)?;
                } else {
                    v.delegated = v.delegated.checked_add(*amount).ok_or(TxError::Overflow)?;
                }
                let slot = self
                    .staking
                    .delegations
                    .entry((*sender, operator))
                    .or_insert(0);
                *slot = slot.checked_add(*amount).ok_or(TxError::Overflow)?;
                Ok(StakeDebit::Amount(*amount))
            }
            TxBody::Undelegate { validator, amount } => {
                if *amount == 0 {
                    return Err(TxError::Malformed("montant nul"));
                }
                let operator = *self
                    .staking
                    .keys
                    .get(validator)
                    .ok_or(TxError::Malformed("validateur inconnu"))?;
                let current = self.staking.delegation(sender, &operator);
                let left = current
                    .checked_sub(*amount)
                    .ok_or(TxError::Malformed("délégation insuffisante"))?;
                if left == 0 {
                    self.staking.delegations.remove(&(*sender, operator));
                } else {
                    self.staking.delegations.insert((*sender, operator), left);
                }
                if let Some(v) = self.staking.validators.get_mut(&operator) {
                    if operator == *sender {
                        v.self_stake = v.self_stake.saturating_sub(*amount);
                    } else {
                        v.delegated = v.delegated.saturating_sub(*amount);
                    }
                }
                let slot = self
                    .staking
                    .unbonding
                    .entry((release, *sender))
                    .or_insert(0);
                *slot = slot.checked_add(*amount).ok_or(TxError::Overflow)?;
                if self.staking.is_active(&operator) {
                    self.staking.dirty = true;
                }
                Ok(StakeDebit::None)
            }
            TxBody::Unjail => {
                let v = self
                    .staking
                    .validators
                    .get_mut(sender)
                    .ok_or(TxError::Malformed("pas de validateur à ce nom"))?;
                let ValidatorStatus::Jailed { since } = v.status else {
                    return Err(TxError::Malformed("validateur non mis à l'écart"));
                };
                if height < since.saturating_add(JAIL_MIN_BLOCKS) {
                    return Err(TxError::Malformed("mise à l'écart pas encore terminée"));
                }
                if v.self_stake < MIN_VALIDATOR_STAKE {
                    return Err(TxError::Malformed("stake propre insuffisant"));
                }
                v.status = ValidatorStatus::Candidate;
                v.missed_streak = 0;
                Ok(StakeDebit::None)
            }
            TxBody::Leave => {
                let v = self
                    .staking
                    .validators
                    .get_mut(sender)
                    .ok_or(TxError::Malformed("pas de validateur à ce nom"))?;
                if matches!(v.status, ValidatorStatus::Left | ValidatorStatus::Banned) {
                    return Err(TxError::Malformed("validateur déjà parti ou exclu"));
                }
                v.status = ValidatorStatus::Left;
                self.staking.unbond_all(sender, release)?;
                self.staking.dirty = true;
                Ok(StakeDebit::None)
            }
            TxBody::SignalUpgrade { version } => {
                let v = self
                    .staking
                    .validators
                    .get_mut(sender)
                    .ok_or(TxError::Malformed("pas de validateur à ce nom"))?;
                v.signaled_version = *version;
                Ok(StakeDebit::None)
            }
            TxBody::ReportEquivocation { evidence } => {
                self.punish_equivocation(sender, height, evidence)?;
                Ok(StakeDebit::None)
            }
            TxBody::Transfer { .. }
            | TxBody::BatchTransfer { .. }
            | TxBody::ConfigureMultisig { .. } => {
                Err(TxError::Malformed("pas une transaction de validation"))
            }
        }
    }

    /// Double signature prouvée : stake propre confisqué (y compris en
    /// restitution) et 1 % des délégations ; 10 % à celui qui prouve, le
    /// reste fondu ; exclusion définitive ; les délégations restantes sont
    /// mises en restitution.
    fn punish_equivocation(
        &mut self,
        reporter: &Address,
        height: u64,
        bytes: &[u8],
    ) -> Result<(), TxError> {
        let evidence: Evidence =
            decode(bytes).map_err(|_| TxError::Malformed("preuve illisible"))?;
        let id = evidence.id();
        if self.staking.evidence_seen.contains(&id) {
            return Err(TxError::Malformed("faute déjà sanctionnée"));
        }
        let at = evidence.height();
        if at >= height || at < height.saturating_sub(UNBONDING_BLOCKS) {
            return Err(TxError::Malformed("preuve trop ancienne ou future"));
        }
        let keys = self.staking.set_at(at);
        evidence
            .verify(&self.chain_id, &keys)
            .map_err(|_| TxError::Malformed("preuve invalide"))?;
        let index = evidence
            .validator(keys.len())
            .ok_or(TxError::Malformed("preuve invalide"))?;
        let key = keys
            .get(usize::from(index))
            .ok_or(TxError::Malformed("preuve invalide"))?;
        let operator = *self
            .staking
            .keys
            .get(key)
            .ok_or(TxError::Malformed("validateur inconnu"))?;

        // Stake propre, délégué ou en restitution.
        let mut slashed = self.staking.delegation(&operator, &operator);
        self.staking.delegations.remove(&(operator, operator));
        let pending: Vec<(u64, Address)> = self
            .staking
            .unbonding
            .keys()
            .filter(|(_, a)| *a == operator)
            .copied()
            .collect();
        for k in pending {
            let amount = self.staking.unbonding.remove(&k).unwrap_or(0);
            slashed = slashed.checked_add(amount).ok_or(TxError::Overflow)?;
        }
        // Délégants : 1 % de leur délégation, pour inciter à bien choisir.
        let delegations: Vec<(Address, Amount)> = self
            .staking
            .delegations
            .iter()
            .filter(|((_, o), _)| *o == operator)
            .map(|((d, _), a)| (*d, *a))
            .collect();
        for (delegator, amount) in delegations {
            let cut = pro_rata(amount, DELEGATOR_SLASH_PERCENT, 100);
            self.staking
                .delegations
                .insert((delegator, operator), amount.saturating_sub(cut));
            slashed = slashed.checked_add(cut).ok_or(TxError::Overflow)?;
        }
        self.staking
            .unbond_all(&operator, height.saturating_add(UNBONDING_BLOCKS))?;
        if let Some(v) = self.staking.validators.get_mut(&operator) {
            v.status = ValidatorStatus::Banned;
        }
        self.staking.evidence_seen.insert(id);
        self.staking.dirty = true;

        let reward = pro_rata(slashed, REPORTER_REWARD_PERCENT, 100);
        let melted = slashed.saturating_sub(reward);
        let a = self.accounts.entry(*reporter).or_default();
        a.balance = a.balance.checked_add(reward).ok_or(TxError::Overflow)?;
        self.supply = self.supply.checked_sub(melted).ok_or(TxError::Overflow)?;
        Ok(())
    }

    /// Stake total bloqué : délégations et restitutions en cours.
    pub(crate) fn locked_stake(&self) -> Option<Amount> {
        self.staking
            .delegations
            .values()
            .chain(self.staking.unbonding.values())
            .try_fold(0u64, |acc, v| acc.checked_add(*v))
    }
}
