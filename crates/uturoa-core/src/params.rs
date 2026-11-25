//! Paramètres du protocole. Il n'existe aucune clé d'administration : ils ne
//! changent que par une nouvelle version de protocole.

use crate::Amount;
use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Params {
    /// Plancher des frais d'une transaction (SPEC §7, formule dynamique au lot 2).
    pub min_fee: Amount,
    pub max_block_txs: u64,
    /// Nombre maximal de parts de frappe par bloc (SPEC §5, à régler au lot 3).
    pub max_block_shares: u64,
    pub max_block_bytes: u64,
    pub timeout_propose_ms: u64,
    pub timeout_propose_delta_ms: u64,
    pub timeout_vote_ms: u64,
    pub timeout_vote_delta_ms: u64,
    /// Intervalle minimal entre deux blocs : 2 s visées (SPEC §4).
    pub min_block_interval_ms: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            min_fee: 10_000,
            max_block_txs: 5_000,
            max_block_shares: 500,
            max_block_bytes: 4 * 1024 * 1024,
            timeout_propose_ms: 3_000,
            timeout_propose_delta_ms: 1_000,
            timeout_vote_ms: 1_000,
            timeout_vote_delta_ms: 500,
            min_block_interval_ms: 2_000,
        }
    }
}

/// Dérive d'horloge maximale tolérée pour l'horodatage d'un bloc (ms).
pub const MAX_CLOCK_DRIFT_MS: u64 = 15_000;
