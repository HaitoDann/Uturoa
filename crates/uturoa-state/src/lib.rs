//! Machine d'état d'Uturoa (SPEC §3, §6, §7, §8).
//!
//! Ce lot couvre les comptes, les envois, les multisignatures, les frais
//! dynamiques, le melt, la frappe de la phase 1 (parts RandomX partagées par
//! période), la forge, et la preuve d'enjeu (validateurs, délégation,
//! présence, sanctions, récompenses, bascule).

mod fees;
pub mod mint;
pub mod staking;
mod state;

pub use fees::{next_base_fee, split_fee};
pub use mint::{MintState, PowHasher, ShareError, meets_target};
pub use staking::{Staking, ValidatorRecord, ValidatorStatus};
pub use state::{BlockError, BlockInput, State, TxError, block_issuance};
