//! Machine d'état d'Uturoa (SPEC §3, §6, §7, §8).
//!
//! Ce lot couvre les comptes, les envois, les multisignatures, les frais
//! dynamiques, le melt et l'émission (frappe puis forge). Les unités émises
//! sont mises de côté dans `pending_rewards` : leur partage entre les parts de
//! frappe (lot 3) et les validateurs (lot 4) viendra ensuite.

mod fees;
mod state;

pub use fees::{next_base_fee, split_fee};
pub use state::{BlockError, State, TxError, block_issuance};
