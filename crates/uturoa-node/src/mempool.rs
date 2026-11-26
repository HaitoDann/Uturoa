//! Transactions et parts de frappe en attente d'un bloc.
//!
//! Les transactions sont rangées par émetteur et par nonce. À la
//! construction d'un bloc, elles sont essayées une à une sur une copie de
//! l'état : celles qui échouent sont simplement écartées, si bien qu'un bloc
//! construit est toujours valide.
//!
//! La preuve de travail d'une part est vérifiée une seule fois, à son
//! arrivée ; à la construction, seules les vérifications bon marché sont
//! refaites (ancrage, unicité).

use std::collections::{BTreeMap, BTreeSet};
use uturoa_core::{Address, Hash, MintShare, SignedTransaction};
use uturoa_state::{PowHasher, State, meets_target};

/// Transactions en attente, au plus.
pub const MAX_TXS: usize = 20_000;
/// Transactions en attente par émetteur, au plus.
pub const MAX_PER_SENDER: usize = 64;
/// Parts en attente, au plus.
pub const MAX_SHARES: usize = 2_000;

#[derive(Default)]
pub struct Mempool {
    txs: BTreeMap<Address, BTreeMap<u64, SignedTransaction>>,
    count: usize,
    shares: BTreeMap<Hash, MintShare>,
    rejected_shares: BTreeSet<Hash>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddError {
    Full,
    Invalid(String),
    Duplicate,
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddError::Full => write!(f, "file d'attente pleine"),
            AddError::Invalid(e) => write!(f, "{e}"),
            AddError::Duplicate => write!(f, "déjà en attente"),
        }
    }
}

impl Mempool {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn shares_len(&self) -> usize {
        self.shares.len()
    }

    /// Ajoute une transaction. Elle est vérifiée sur une copie de l'état si
    /// c'est la prochaine de son émetteur ; sinon, elle attend son tour.
    pub fn add(&mut self, state: &State, tx: SignedTransaction) -> Result<(), AddError> {
        if self.count >= MAX_TXS {
            return Err(AddError::Full);
        }
        let sender = tx.tx.sender;
        let account_nonce = state.account(&sender).map_or(0, |a| a.nonce);
        if tx.tx.nonce <= account_nonce {
            return Err(AddError::Invalid("nonce déjà utilisé".into()));
        }
        if tx.tx.chain_id != state.chain_id {
            return Err(AddError::Invalid("autre chaîne".into()));
        }
        let queue = self.txs.entry(sender).or_default();
        if queue.contains_key(&tx.tx.nonce) {
            return Err(AddError::Duplicate);
        }
        if queue.len() >= MAX_PER_SENDER {
            return Err(AddError::Full);
        }
        if tx.tx.nonce == account_nonce.saturating_add(1) {
            let mut probe = state.clone();
            probe
                .apply_tx(&tx, &Address::from_bytes([0; 20]))
                .map_err(|e| AddError::Invalid(e.to_string()))?;
        }
        queue.insert(tx.tx.nonce, tx);
        self.count = self.count.saturating_add(1);
        Ok(())
    }

    /// Ajoute une part de frappe, après avoir vérifié sa preuve de travail.
    pub fn add_share(
        &mut self,
        state: &State,
        parent_hash: &Hash,
        share: MintShare,
        pow: &dyn PowHasher,
    ) -> Result<(), AddError> {
        let id = share.id();
        if self.shares.contains_key(&id) || self.rejected_shares.contains(&id) {
            return Err(AddError::Duplicate);
        }
        if self.shares.len() >= MAX_SHARES {
            return Err(AddError::Full);
        }
        let height = state.height.saturating_add(1);
        state
            .mint()
            .precheck(&state.chain_id, height, parent_hash, &share)
            .map_err(|e| AddError::Invalid(e.to_string()))?;
        let seed = state
            .mint()
            .seed_for(height)
            .ok_or_else(|| AddError::Invalid("clé RandomX inconnue".into()))?;
        if !meets_target(
            &pow.hash(&seed, &share.pow_input()),
            state.mint().difficulty,
        ) {
            self.rejected_shares.insert(id);
            return Err(AddError::Invalid("travail insuffisant".into()));
        }
        self.shares.insert(id, share);
        Ok(())
    }

    /// Transactions à proposer, dans un ordre valide, essayées sur une copie
    /// de l'état. Au plus `max` transactions.
    pub fn select_txs(&self, state: &State, max: usize) -> Vec<SignedTransaction> {
        let mut probe = state.clone();
        let proposer = Address::from_bytes([0; 20]);
        let mut out = Vec::new();
        'senders: for queue in self.txs.values() {
            for tx in queue.values() {
                if out.len() >= max {
                    break 'senders;
                }
                if probe.apply_tx(tx, &proposer).is_ok() {
                    out.push(tx.clone());
                } else {
                    // Un trou de nonce bloque les suivantes de cet émetteur.
                    continue 'senders;
                }
            }
        }
        out
    }

    /// Parts à proposer : ancrage encore valide, jamais acceptées.
    pub fn select_shares(&self, state: &State, parent_hash: &Hash, max: usize) -> Vec<MintShare> {
        let height = state.height.saturating_add(1);
        self.shares
            .values()
            .filter(|s| {
                state
                    .mint()
                    .precheck(&state.chain_id, height, parent_hash, s)
                    .is_ok()
            })
            .take(max)
            .cloned()
            .collect()
    }

    /// Après un bloc validé : retire ce qui est inclus ou périmé.
    pub fn prune(&mut self, state: &State, parent_hash: &Hash) {
        let mut count = 0usize;
        self.txs.retain(|sender, queue| {
            let nonce = state.account(sender).map_or(0, |a| a.nonce);
            queue.retain(|n, _| *n > nonce);
            count = count.saturating_add(queue.len());
            !queue.is_empty()
        });
        self.count = count;
        let height = state.height.saturating_add(1);
        self.shares.retain(|_, s| {
            state
                .mint()
                .precheck(&state.chain_id, height, parent_hash, s)
                .is_ok()
        });
        if self.rejected_shares.len() > MAX_SHARES {
            self.rejected_shares.clear();
        }
    }
}
