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
use uturoa_node::service::{NodeConfig, launch};
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
        /// Intervalle entre deux blocs, en millisecondes (12 000 par défaut).
        #[arg(long)]
        block_interval_ms: Option<u64>,
        /// Premier port réseau ; le validateur i écoute sur port + i - 1.
        #[arg(long, default_value_t = 30_333)]
        base_port: u16,
    },
    /// Lance un nœud : validateur avec --key, observateur sans.
    Run {
        #[arg(long)]
        genesis: PathBuf,
        #[arg(long)]
        data_dir: PathBuf,
        /// Fichier de clé de validateur (mot de passe : UTUROA_KEY_PASSWORD).
        #[arg(long)]
        key: Option<PathBuf>,
        #[arg(long, default_value = "/ip4/0.0.0.0/tcp/30333")]
        listen: String,
        /// Pair à joindre, répétable : /ip4/IP/tcp/PORT/p2p/ID.
        #[arg(long)]
        peer: Vec<String>,
        /// Empreinte (hex) d'un bloc de confiance, pour accepter un instantané
        /// au-delà de la phase d'amorçage.
        #[arg(long)]
        trusted_block: Option<String>,
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
            block_interval_ms,
            base_port,
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
                block_interval_ms,
            };
            let path = dir.join("genesis.json");
            genesis.save(&path)?;
            println!("Réseau local créé dans {}", dir.display());
            println!("  genèse : {}", path.display());
            println!("  empreinte : {}", hash_to_hex(&genesis.hash()?));
            println!(
                "  {validators} validateur(s) : validator-1.json … validator-{validators}.json"
            );
            // Identités réseau et commandes de lancement.
            let mut addrs = Vec::with_capacity(validators);
            for i in 1..=validators {
                let id = uturoa_node::service::peer_id(&dir.join(format!("d{i}")))?;
                let offset = u16::try_from(i.saturating_sub(1)).unwrap_or(0);
                let port = base_port.saturating_add(offset);
                addrs.push((port, format!("/ip4/127.0.0.1/tcp/{port}/p2p/{id}")));
            }
            println!();
            println!("Pour lancer le validateur i (un terminal chacun) :");
            for (i, (port, _)) in addrs.iter().enumerate() {
                let peers: String = addrs
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, (_, a))| format!(" --peer {a}"))
                    .collect();
                let n = i.saturating_add(1);
                println!(
                    "  uturoa-node run --genesis {g} --key {d}/validator-{n}.json --data-dir {d}/d{n} --listen /ip4/127.0.0.1/tcp/{port}{peers}",
                    g = path.display(),
                    d = dir.display()
                );
            }
        }
        Command::Run {
            genesis,
            data_dir,
            key,
            listen,
            peer,
            trusted_block,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info,libp2p=warn".into()),
                )
                .init();
            let genesis = GenesisFile::load(&genesis)?;
            let key = match key {
                Some(path) => Some(KeyFile::load(&path)?.decrypt(&password(None)?)?),
                None => None,
            };
            let trusted_block = match trusted_block {
                Some(hex) => Some(uturoa_keystore::encoding::hash_from_hex(&hex)?),
                None => None,
            };
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async move {
                let node = launch(
                    NodeConfig {
                        genesis,
                        data_dir,
                        key,
                        listen: listen.parse()?,
                        peers: peer.iter().map(|p| p.parse()).collect::<Result<_, _>>()?,
                        trusted_block,
                    },
                    Box::new(RandomXHasher::new()),
                )?;
                println!("nœud démarré : {}", node.peer_id);
                tokio::signal::ctrl_c().await?;
                let _ = node.shutdown.send(true);
                for t in node.tasks {
                    let _ = t.await;
                }
                anyhow::Ok(())
            })?;
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
