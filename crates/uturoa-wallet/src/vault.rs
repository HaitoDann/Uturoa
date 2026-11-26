//! Coffre du portefeuille : phrase de récupération chiffrée et comptes.
//!
//! - Une phrase de 12 mots (BIP-39) est créée à la première ouverture. Elle
//!   seule permet de tout restaurer sur un autre poste.
//! - Compte `i` = clé Ed25519 de `BLAKE3-derive_key(DERIVATION, graine ‖ i)`,
//!   `graine` étant la graine BIP-39 de la phrase, sans phrase secrète.
//! - `portefeuille.json` garde la phrase chiffrée (Argon2id +
//!   XChaCha20-Poly1305), les libellés des comptes et l'adresse du nœud.

use anyhow::{Context, Result, anyhow, bail};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uturoa_crypto::KeyPair;
use uturoa_keystore::keyfile::{Kdf, Sealed, write_private};

pub const VAULT_FILE: &str = "portefeuille.json";
pub const DERIVATION: &str = "Uturoa portefeuille v1 compte ed25519";
/// Nœud par défaut : l'API d'un nœud local.
pub const DEFAULT_NODE: &str = "127.0.0.1:8545";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Vault {
    pub version: u8,
    pub node: String,
    pub phrase: Sealed,
    pub accounts: Vec<AccountEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountEntry {
    pub label: String,
}

/// Nouvelle phrase de 12 mots, tirée de l'aléa du système.
pub fn new_phrase() -> Result<String> {
    let mut entropy = [0u8; 16];
    OsRng.fill_bytes(&mut entropy);
    Ok(bip39::Mnemonic::from_entropy(&entropy)
        .map_err(|e| anyhow!("{e}"))?
        .to_string())
}

/// Graine BIP-39 d'une phrase ; refuse une phrase invalide (mot inconnu,
/// somme de contrôle fausse).
pub fn seed_of(phrase: &str) -> Result<[u8; 64]> {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    let normalized = words.join(" ").to_lowercase();
    let mnemonic = bip39::Mnemonic::parse_normalized(&normalized)
        .map_err(|_| anyhow!("phrase de récupération invalide"))?;
    if mnemonic.word_count() != 12 {
        bail!("la phrase doit compter 12 mots");
    }
    Ok(mnemonic.to_seed(""))
}

/// Clé du compte `index`.
pub fn derive_account(seed: &[u8; 64], index: u32) -> KeyPair {
    let mut input = seed.to_vec();
    input.extend_from_slice(&index.to_le_bytes());
    KeyPair::from_secret_bytes(&blake3::derive_key(DERIVATION, &input))
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(VAULT_FILE)
}

impl Vault {
    pub fn create(phrase: &str, password: &str, kdf: Kdf) -> Result<Self> {
        seed_of(phrase)?;
        if password.chars().count() < 8 {
            bail!("le mot de passe doit faire au moins 8 caractères");
        }
        Ok(Self {
            version: 1,
            node: DEFAULT_NODE.to_owned(),
            phrase: Sealed::seal(phrase.trim().as_bytes(), password, kdf)?,
            accounts: vec![AccountEntry {
                label: "Compte principal".into(),
            }],
        })
    }

    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let p = path(dir);
        if !p.exists() {
            return Ok(None);
        }
        let text =
            std::fs::read_to_string(&p).with_context(|| format!("lecture de {}", p.display()))?;
        Ok(Some(
            serde_json::from_str(&text).context("portefeuille illisible")?,
        ))
    }

    /// Enregistre de façon atomique : fichier temporaire, puis renommage. Une
    /// coupure pendant l'écriture laisse l'ancien fichier intact.
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{VAULT_FILE}.tmp"));
        if tmp.exists() {
            std::fs::remove_file(&tmp)?;
        }
        write_private(&tmp, serde_json::to_string_pretty(self)?.as_bytes())?;
        std::fs::rename(&tmp, path(dir)).context("enregistrement du portefeuille")?;
        Ok(())
    }

    /// Déchiffre la phrase et renvoie la graine.
    pub fn unlock(&self, password: &str) -> Result<[u8; 64]> {
        let bytes = self
            .phrase
            .open(password)
            .map_err(|_| anyhow!("mot de passe incorrect"))?;
        let phrase = String::from_utf8(bytes).map_err(|_| anyhow!("phrase illisible"))?;
        seed_of(&phrase)
    }

    /// La phrase en clair, pour la noter à nouveau.
    pub fn reveal(&self, password: &str) -> Result<String> {
        let bytes = self
            .phrase
            .open(password)
            .map_err(|_| anyhow!("mot de passe incorrect"))?;
        String::from_utf8(bytes).map_err(|_| anyhow!("phrase illisible"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uturoa_keystore::keyfile::DEFAULT_KDF;

    const FAST: Kdf = Kdf {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };

    #[test]
    fn phrase_and_derivation_are_deterministic() {
        let phrase = new_phrase().unwrap();
        assert_eq!(phrase.split_whitespace().count(), 12);
        let seed = seed_of(&phrase).unwrap();
        // Espaces et majuscules en trop ne changent rien.
        let messy = format!("  {}  ", phrase.to_uppercase().replace(' ', "   "));
        assert_eq!(seed_of(&messy).unwrap(), seed);
        assert_eq!(
            derive_account(&seed, 0).public_key(),
            derive_account(&seed, 0).public_key()
        );
        assert_ne!(
            derive_account(&seed, 0).public_key(),
            derive_account(&seed, 1).public_key()
        );
        assert!(seed_of("pas une vraie phrase du tout").is_err());
        let _ = DEFAULT_KDF;
    }

    #[test]
    fn vault_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let phrase = new_phrase().unwrap();
        assert!(Vault::create(&phrase, "court", FAST).is_err());
        let v = Vault::create(&phrase, "motdepasse", FAST).unwrap();
        v.save(dir.path()).unwrap();
        // Un second enregistrement remplace le premier.
        v.save(dir.path()).unwrap();
        let back = Vault::load(dir.path()).unwrap().unwrap();
        assert_eq!(
            back.unlock("motdepasse").unwrap(),
            seed_of(&phrase).unwrap()
        );
        assert!(back.unlock("mauvais").is_err());
        assert_eq!(back.reveal("motdepasse").unwrap(), phrase);
    }
}
