//! État, application des transactions et des blocs.

use crate::fees::{next_base_fee, split_fee};
use std::collections::BTreeMap;
use thiserror::Error;
use uturoa_core::{
    Account, Address, Amount, Authorization, CoreError, Hash, MAX_BATCH_PAYMENTS, MAX_SUPPLY,
    MIN_BASE_FEE_PER_BYTE, MIN_RESERVE, MINT_PER_BLOCK, MINT_PHASE_BLOCKS, Params,
    SignedTransaction, TxBody, encode, forge_amount,
};
use uturoa_crypto::{Domain, hash_tagged, merkle_root};

/// Raison du refus d'une transaction.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TxError {
    #[error("identifiant de chaîne incorrect")]
    WrongChain,
    #[error("nonce attendu {expected}, reçu {got}")]
    BadNonce { expected: u64, got: u64 },
    #[error("autorisation invalide : {0}")]
    Unauthorized(&'static str),
    #[error("frais insuffisants : {required} unités requises, {offered} offertes")]
    FeeTooLow { required: Amount, offered: Amount },
    #[error("solde insuffisant")]
    InsufficientBalance,
    #[error("le solde restant doit être nul ou d'au moins la réserve minimale")]
    BelowReserve,
    #[error("un nouveau compte doit recevoir au moins la réserve minimale")]
    NewAccountBelowReserve,
    #[error("transaction mal formée : {0}")]
    Malformed(&'static str),
    #[error("dépassement arithmétique")]
    Overflow,
    #[error("transaction pas encore prise en charge")]
    NotYetSupported,
}

/// Raison du refus d'un bloc.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlockError {
    #[error("hauteur attendue {expected}, reçue {got}")]
    BadHeight { expected: u64, got: u64 },
    #[error("transaction {index} invalide : {error}")]
    InvalidTx { index: usize, error: TxError },
    #[error("invariant de l'offre violé")]
    Invariant,
}

impl From<CoreError> for TxError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::InvalidApprovals(m) => TxError::Unauthorized(m),
            _ => TxError::Unauthorized("signature invalide"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub chain_id: String,
    pub protocol_version: u32,
    /// Hauteur du dernier bloc appliqué (0 : genèse).
    pub height: u64,
    pub params: Params,
    accounts: BTreeMap<Address, Account>,
    /// Offre en circulation : émise moins fondue. Jamais plus que `MAX_SUPPLY`.
    supply: Amount,
    /// Unités émises, en attente de partage (lots 3 et 4).
    pending_rewards: Amount,
    /// Frais de base par octet pour le prochain bloc.
    base_fee_per_byte: Amount,
}

/// Unités émises par le bloc `height`, `supply` étant l'offre avant le bloc :
/// la frappe pendant la phase 1, la forge ensuite. Jamais au-delà du plafond.
pub fn block_issuance(height: u64, supply: Amount) -> Amount {
    let room = MAX_SUPPLY.saturating_sub(supply);
    let amount = if height <= MINT_PHASE_BLOCKS {
        MINT_PER_BLOCK
    } else {
        forge_amount(supply)
    };
    amount.min(room)
}

impl State {
    /// État de genèse : aucun compte, aucune unité.
    pub fn genesis(chain_id: impl Into<String>, params: Params) -> Self {
        Self {
            chain_id: chain_id.into(),
            protocol_version: 1,
            height: 0,
            params,
            accounts: BTreeMap::new(),
            supply: 0,
            pending_rewards: 0,
            base_fee_per_byte: MIN_BASE_FEE_PER_BYTE,
        }
    }

    pub fn account(&self, address: &Address) -> Option<&Account> {
        self.accounts.get(address)
    }

    pub fn balance(&self, address: &Address) -> Amount {
        self.accounts.get(address).map_or(0, |a| a.balance)
    }

    pub fn supply(&self) -> Amount {
        self.supply
    }

    pub fn pending_rewards(&self) -> Amount {
        self.pending_rewards
    }

    pub fn base_fee_per_byte(&self) -> Amount {
        self.base_fee_per_byte
    }

    /// Frais minimaux d'une transaction signée : frais de base × taille.
    pub fn required_fee(&self, stx: &SignedTransaction) -> Amount {
        let len = Amount::try_from(encode(stx).len()).unwrap_or(Amount::MAX);
        self.base_fee_per_byte.saturating_mul(len)
    }

    /// Somme des soldes et des récompenses en attente égale à l'offre, sous le plafond.
    pub fn check_invariant(&self) -> bool {
        let balances = self
            .accounts
            .values()
            .try_fold(0u64, |acc, a| acc.checked_add(a.balance));
        balances.and_then(|b| b.checked_add(self.pending_rewards)) == Some(self.supply)
            && self.supply <= MAX_SUPPLY
    }

    /// Racine d'état : comptes triés par adresse, puis variables globales.
    pub fn state_root(&self) -> Hash {
        let leaves: Vec<Hash> = self
            .accounts
            .iter()
            .map(|(address, account)| hash_tagged(Domain::State, &encode(&(address, account))))
            .collect();
        let globals = (
            &self.chain_id,
            self.protocol_version,
            self.height,
            &self.params,
            self.supply,
            self.pending_rewards,
            self.base_fee_per_byte,
            merkle_root(&leaves),
        );
        hash_tagged(Domain::State, &encode(&globals))
    }

    /// Applique un bloc de façon atomique : en cas d'erreur, l'état ne change pas.
    pub fn apply_block(
        &mut self,
        height: u64,
        txs: &[SignedTransaction],
        proposer: &Address,
        block_bytes: u64,
    ) -> Result<(), BlockError> {
        let expected = self.height.saturating_add(1);
        if height != expected {
            return Err(BlockError::BadHeight {
                expected,
                got: height,
            });
        }
        let mut next = self.clone();
        for (index, stx) in txs.iter().enumerate() {
            next.apply_tx(stx, proposer)
                .map_err(|error| BlockError::InvalidTx { index, error })?;
        }
        let issued = block_issuance(height, next.supply);
        next.supply = next
            .supply
            .checked_add(issued)
            .ok_or(BlockError::Invariant)?;
        next.pending_rewards = next
            .pending_rewards
            .checked_add(issued)
            .ok_or(BlockError::Invariant)?;
        next.base_fee_per_byte = next_base_fee(
            next.base_fee_per_byte,
            block_bytes,
            next.params.max_block_bytes,
        );
        next.height = height;
        if !next.check_invariant() {
            return Err(BlockError::Invariant);
        }
        *self = next;
        Ok(())
    }

    /// Applique une transaction. En cas d'erreur, l'état ne change pas.
    pub fn apply_tx(&mut self, stx: &SignedTransaction, proposer: &Address) -> Result<(), TxError> {
        let tx = &stx.tx;
        if tx.chain_id != self.chain_id {
            return Err(TxError::WrongChain);
        }
        let mut sender = self.accounts.get(&tx.sender).cloned().unwrap_or_default();

        // Autorisation.
        let new_key = self.authorize(stx, &sender)?;

        // Nonce.
        let expected = sender.nonce.checked_add(1).ok_or(TxError::Overflow)?;
        if tx.nonce != expected {
            return Err(TxError::BadNonce {
                expected,
                got: tx.nonce,
            });
        }

        // Frais.
        let required = self.required_fee(stx);
        if tx.fee < required {
            return Err(TxError::FeeTooLow {
                required,
                offered: tx.fee,
            });
        }

        // Effets du corps, calculés avant toute écriture.
        let mut credits: BTreeMap<Address, Amount> = BTreeMap::new();
        let mut debit = tx.fee;
        match &tx.body {
            TxBody::Transfer { to, amount, .. } => {
                if !tx.body.memos_fit() {
                    return Err(TxError::Malformed("mémo trop long"));
                }
                add_credit(&mut credits, &tx.sender, to, *amount)?;
                debit = debit.checked_add(*amount).ok_or(TxError::Overflow)?;
            }
            TxBody::BatchTransfer { payments } => {
                if payments.is_empty() || payments.len() > MAX_BATCH_PAYMENTS {
                    return Err(TxError::Malformed("nombre de paiements invalide"));
                }
                if !tx.body.memos_fit() {
                    return Err(TxError::Malformed("mémo trop long"));
                }
                for p in payments {
                    add_credit(&mut credits, &tx.sender, &p.to, p.amount)?;
                    debit = debit.checked_add(p.amount).ok_or(TxError::Overflow)?;
                }
            }
            TxBody::ConfigureMultisig { multisig } => {
                if multisig.as_ref().is_some_and(|m| !m.is_valid()) {
                    return Err(TxError::Malformed("règle multisignature invalide"));
                }
            }
            TxBody::Delegate { .. }
            | TxBody::Undelegate { .. }
            | TxBody::RegisterValidator { .. }
            | TxBody::UpdateValidatorInfo { .. }
            | TxBody::Unjail
            | TxBody::Leave
            | TxBody::ReportEquivocation { .. }
            | TxBody::SignalUpgrade { .. } => return Err(TxError::NotYetSupported),
        }

        // Solde de l'émetteur.
        let remaining = sender
            .balance
            .checked_sub(debit)
            .ok_or(TxError::InsufficientBalance)?;
        if remaining != 0 && remaining < MIN_RESERVE {
            return Err(TxError::BelowReserve);
        }

        // Destinataires : un compte neuf doit recevoir au moins la réserve.
        let mut updated: Vec<(Address, Account)> = Vec::with_capacity(credits.len());
        for (to, amount) in &credits {
            let mut account = self.accounts.get(to).cloned().unwrap_or_default();
            if !self.accounts.contains_key(to) && *amount < MIN_RESERVE {
                return Err(TxError::NewAccountBelowReserve);
            }
            account.balance = account
                .balance
                .checked_add(*amount)
                .ok_or(TxError::Overflow)?;
            updated.push((*to, account));
        }

        // Frais : melt et part du proposeur.
        let (melt, to_proposer) = split_fee(tx.fee);
        let supply = self.supply.checked_sub(melt).ok_or(TxError::Overflow)?;

        // Écriture.
        sender.balance = remaining;
        sender.nonce = expected;
        if let Some(key) = new_key {
            sender.key = Some(key);
        }
        if let TxBody::ConfigureMultisig { multisig } = &tx.body {
            sender.multisig = multisig.clone();
        }
        self.accounts.insert(tx.sender, sender);
        for (address, account) in updated {
            self.accounts.insert(address, account);
        }
        if to_proposer > 0 {
            let p = self.accounts.entry(*proposer).or_default();
            p.balance = p
                .balance
                .checked_add(to_proposer)
                .ok_or(TxError::Overflow)?;
        }
        self.supply = supply;
        Ok(())
    }

    /// Vérifie l'autorisation. Renvoie la clé à enregistrer lors d'une première
    /// dépense.
    fn authorize(
        &self,
        stx: &SignedTransaction,
        sender: &Account,
    ) -> Result<Option<uturoa_core::PublicKey>, TxError> {
        if let Some(multisig) = &sender.multisig {
            stx.verify_multisig(multisig)?;
            return Ok(None);
        }
        match (&stx.auth, &sender.key) {
            (
                Authorization::Single {
                    public_key: None, ..
                },
                Some(key),
            ) => {
                stx.verify_single(key)?;
                Ok(None)
            }
            (
                Authorization::Single {
                    public_key: Some(_),
                    ..
                },
                Some(_),
            ) => Err(TxError::Unauthorized("clé déjà connue : ne pas la joindre")),
            (
                Authorization::Single {
                    public_key: Some(key),
                    ..
                },
                None,
            ) => {
                if Address::from_public_key(key) != stx.tx.sender {
                    return Err(TxError::Unauthorized(
                        "la clé ne correspond pas à l'adresse",
                    ));
                }
                stx.verify_single(key)?;
                Ok(Some(key.clone()))
            }
            (
                Authorization::Single {
                    public_key: None, ..
                },
                None,
            ) => Err(TxError::Unauthorized(
                "première dépense : clé publique requise",
            )),
            (Authorization::Multi { .. }, _) => {
                Err(TxError::Unauthorized("compte sans multisignature"))
            }
        }
    }

    /// Crédite directement un compte, en ajoutant à l'offre. Réservé aux tests
    /// et à la construction d'états d'essai, jamais au protocole.
    #[doc(hidden)]
    pub fn credit_for_tests(&mut self, address: &Address, amount: Amount) {
        let a = self.accounts.entry(*address).or_default();
        a.balance = a.balance.saturating_add(amount);
        self.supply = self.supply.saturating_add(amount);
    }
}

fn add_credit(
    credits: &mut BTreeMap<Address, Amount>,
    sender: &Address,
    to: &Address,
    amount: Amount,
) -> Result<(), TxError> {
    if amount == 0 {
        return Err(TxError::Malformed("montant nul"));
    }
    if to == sender {
        return Err(TxError::Malformed("envoi à soi-même"));
    }
    let slot = credits.entry(*to).or_insert(0);
    *slot = slot.checked_add(amount).ok_or(TxError::Overflow)?;
    Ok(())
}
