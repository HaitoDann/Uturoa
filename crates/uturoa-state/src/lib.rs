//! Machine d'état d'Uturoa (SPEC §3, §6, §7, §8).
//!
//! Ce lot couvre les comptes, les envois, les multisignatures, les frais
//! dynamiques, le melt, la frappe de la phase 1 (parts RandomX partagées par
//! période) et la forge. Les unités forgées sont mises de côté dans
//! `pending_rewards` : leur partage entre validateurs viendra au lot 4.

mod fees;
pub mod mint;
mod state;

pub use fees::{next_base_fee, split_fee};
pub use mint::{MintState, PowHasher, ShareError, meets_target};
pub use state::{BlockError, BlockInput, State, TxError, block_issuance};
