//! Types du protocole : montants, comptes, transactions, parts de frappe,
//! blocs, votes et paramètres.

pub mod account;
pub mod amount;
pub mod block;
pub mod consensus;
pub mod mint;
pub mod params;
pub mod transaction;

pub use account::{Account, MAX_MULTISIG_SIGNERS, Multisig};
pub use amount::{
    Amount, BLOCKS_PER_YEAR, FORGE_DIVISOR, MAX_SUPPLY, MELT_PERCENT, MIN_BASE_FEE_PER_BYTE,
    MIN_RESERVE, MINT_PER_BLOCK, MINT_PHASE_BLOCKS, MINT_PHASE_TOTAL, TARGET_BLOCK_MS,
    UNITS_PER_COIN, forge_amount,
};
pub use block::{Block, BlockHeader};
pub use consensus::{CommitCertificate, Vote, VoteKind, quorum};
pub use mint::MintShare;
pub use params::Params;
pub use transaction::{
    Approval, Authorization, MAX_BATCH_PAYMENTS, MAX_MEMO_BYTES, Payment, SignedTransaction,
    Transaction, TxBody, ValidatorInfo,
};
pub use uturoa_crypto::{Address, Hash, PublicKey, Signature};

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("encodage invalide")]
    Encoding,
    #[error("signature invalide")]
    InvalidSignature,
    #[error("approbations invalides : {0}")]
    InvalidApprovals(&'static str),
    #[error("certificat de commit invalide : {0}")]
    InvalidCertificate(&'static str),
}

/// Encodage borsh canonique d'une valeur.
pub fn encode<T: borsh::BorshSerialize>(value: &T) -> Vec<u8> {
    // L'écriture dans un Vec ne peut pas échouer.
    borsh::to_vec(value).unwrap_or_default()
}

/// Décodage borsh **canonique** : en plus du décodage strict, refuse toute
/// donnée dont le ré-encodage diffère (par exemple une table aux clés en
/// désordre ou en double). À utiliser pour ce qui vient d'un pair et contient
/// des tables, comme un instantané d'état.
pub fn decode_canonical<T: borsh::BorshDeserialize + borsh::BorshSerialize>(
    bytes: &[u8],
) -> Result<T, CoreError> {
    let value: T = decode(bytes)?;
    if encode(&value) != bytes {
        return Err(CoreError::Encoding);
    }
    Ok(value)
}

/// Décodage borsh strict : refuse les octets en trop et toute valeur inconnue.
pub fn decode<T: borsh::BorshDeserialize>(bytes: &[u8]) -> Result<T, CoreError> {
    borsh::from_slice(bytes).map_err(|_| CoreError::Encoding)
}
