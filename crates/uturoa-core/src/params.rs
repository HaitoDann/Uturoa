//! Paramètres du protocole. Il n'existe aucune clé d'administration : ils ne
//! changent que par une nouvelle version de protocole.

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Params {
    pub max_block_txs: u64,
    /// Nombre maximal de parts de frappe par bloc (SPEC §5) : chaque part coûte une vérification RandomX.
    pub max_block_shares: u64,
    pub max_block_bytes: u64,
    /// Nombre maximal de validateurs actifs après la bascule (SPEC §4.4).
    pub max_active_validators: u64,
    pub timeout_propose_ms: u64,
    pub timeout_propose_delta_ms: u64,
    pub timeout_vote_ms: u64,
    pub timeout_vote_delta_ms: u64,
    /// Intervalle minimal entre deux blocs : 12 s visées (SPEC §4).
    pub min_block_interval_ms: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_block_txs: 5_000,
            max_block_shares: 100,
            max_block_bytes: 4 * 1024 * 1024,
            max_active_validators: 21,
            timeout_propose_ms: 3_000,
            timeout_propose_delta_ms: 1_000,
            timeout_vote_ms: 1_000,
            timeout_vote_delta_ms: 500,
            min_block_interval_ms: 12_000,
        }
    }
}

/// Dérive d'horloge maximale tolérée pour l'horodatage d'un bloc (ms).
pub const MAX_CLOCK_DRIFT_MS: u64 = 15_000;
