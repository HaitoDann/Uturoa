//! Fichier de genèse en JSON.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uturoa_core::{Address, Hash, Params, ValidatorInfo};
use uturoa_crypto::{Domain, hash_tagged};
use uturoa_keystore::encoding::{key_from_hex, key_to_hex};
use uturoa_state::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisFile {
    pub chain_id: String,
    /// Horodatage Unix de la genèse, en millisecondes.
    pub genesis_time_ms: u64,
    /// Difficulté initiale de la frappe.
    pub mint_difficulty: u64,
    /// Validateurs d'amorçage de la phase 1.
    pub validators: Vec<GenesisValidator>,
    /// Intervalle minimal entre deux blocs, pour un réseau local accéléré.
    /// Absent : 12 s. L'émission reste comptée en blocs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisValidator {
    /// Adresse de l'opérateur (`utu1…`).
    pub operator: String,
    /// Clé de consensus, en hexadécimal typé.
    pub key: String,
    pub name: String,
}

impl GenesisFile {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        serde_json::from_str(&text).context("genèse illisible")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("écriture de {}", path.display()))
    }

    /// Empreinte de la genèse : sert de `prev_hash` au bloc 1 et de nom de chaîne.
    pub fn hash(&self) -> Result<Hash> {
        let canonical = serde_json::to_vec(self)?;
        Ok(hash_tagged(Domain::Block, &canonical))
    }

    /// État de genèse correspondant.
    pub fn state(&self) -> Result<State> {
        if self.chain_id.is_empty() {
            bail!("identifiant de chaîne vide");
        }
        let mut bootstrap = Vec::with_capacity(self.validators.len());
        for v in &self.validators {
            let operator: Address = v
                .operator
                .parse()
                .map_err(|e| anyhow!("adresse d'opérateur {} : {e}", v.operator))?;
            let key = key_from_hex(&v.key)?;
            let info = ValidatorInfo {
                name: v.name.clone(),
                site: String::new(),
                contact: String::new(),
            };
            bootstrap.push((operator, key, info));
        }
        let mut params = Params::default();
        if let Some(ms) = self.block_interval_ms {
            params.min_block_interval_ms = ms;
        }
        Ok(State::genesis_with_validators(
            self.chain_id.clone(),
            params,
            self.mint_difficulty,
            bootstrap,
        ))
    }
}

/// Fiche d'un validateur d'amorçage à partir de sa clé.
pub fn validator_entry(key: &uturoa_core::PublicKey, name: &str) -> GenesisValidator {
    GenesisValidator {
        operator: Address::from_public_key(key).to_string(),
        key: key_to_hex(key),
        name: name.to_owned(),
    }
}
