//! Ligne de commande du nœud Uturoa.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use uturoa_core::UNITS_PER_COIN;
use uturoa_crypto::KeyPair;
use uturoa_keystore::encoding::hash_to_hex;
use uturoa_keystore::keyfile::{DEFAULT_KDF, KeyFile, password};
use uturoa_node::chain::Chain;
use uturoa_node::genesis_file::{GenesisFile, validator_entry};
use uturoa_pow::RandomXHasher;

#[derive(Parser)]
#[command(name = "uturoa-node", version, about = "Nœud Uturoa")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Crée un réseau local : clés chiffrées des validateurs et genèse.
    /// Le mot de passe vient de UTUROA_KEY_PASSWORD.
    Devnet {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value_t = 4)]
        validators: usize,
        #[arg(long, default_value = "utu-devnet")]
        chain_id: String,
        /// Difficulté initiale de la frappe (1 : chaque part est valide).
        #[arg(long, default_value_t = 1)]
        mint_difficulty: u64,
    },
    /// Affiche l'état d'une base locale.
    Info {
        #[arg(long)]
        genesis: PathBuf,
        #[arg(long)]
        data_dir: PathBuf,
    },
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Devnet {
            dir,
            validators,
            chain_id,
            mint_difficulty,
        } => {
            if !(1..=100).contains(&validators) {
                bail!("entre 1 et 100 validateurs");
            }
            std::fs::create_dir_all(&dir)?;
            let pw = password(None)?;
            let mut entries = Vec::with_capacity(validators);
            for i in 1..=validators {
                let kp = KeyPair::generate();
                KeyFile::encrypt(&kp, &pw, DEFAULT_KDF)?
                    .save(&dir.join(format!("validator-{i}.json")))?;
                entries.push(validator_entry(
                    &kp.public_key(),
                    &format!("validateur-{i}"),
                ));
            }
            let genesis = GenesisFile {
                chain_id,
                genesis_time_ms: now_ms(),
                mint_difficulty,
                validators: entries,
            };
            let path = dir.join("genesis.json");
            genesis.save(&path)?;
            println!("Réseau local créé dans {}", dir.display());
            println!("  genèse : {}", path.display());
            println!("  empreinte : {}", hash_to_hex(&genesis.hash()?));
            println!(
                "  {validators} validateur(s) : validator-1.json … validator-{validators}.json"
            );
        }
        Command::Info { genesis, data_dir } => {
            let genesis = GenesisFile::load(&genesis)?;
            let chain = Chain::open(&data_dir, &genesis, Box::new(RandomXHasher::new()))?;
            let s = chain.state();
            println!("chaîne        : {}", s.chain_id);
            println!("hauteur       : {}", s.height);
            println!("protocole     : {}", s.protocol_version);
            println!(
                "en circulation: {} pièces",
                s.supply().checked_div(UNITS_PER_COIN).unwrap_or(0)
            );
            println!("racine d'état : {}", hash_to_hex(&s.state_root()));
            println!("validateurs   : {}", chain.next_validators().len());
        }
    }
    Ok(())
}
