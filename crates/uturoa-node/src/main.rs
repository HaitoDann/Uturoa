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
use uturoa_pow::{CachedPow, RandomXHasher};

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
        /// Phase de frappe raccourcie, en blocs, pour répéter la bascule.
        /// Réseaux d'essai seulement.
        #[arg(long)]
        mint_phase_blocks: Option<u64>,
        /// Premier port réseau ; en local, le validateur i écoute sur port + i - 1,
        /// et avec --hosts, tous écoutent sur ce port.
        #[arg(long, default_value_t = 30_333)]
        base_port: u16,
        /// Adresses IP des machines, séparées par des virgules : une par
        /// validateur. Génère aussi un service systemd par machine.
        #[arg(long, value_delimiter = ',')]
        hosts: Vec<String>,
        /// Noms des validateurs, séparés par des virgules.
        #[arg(long, value_delimiter = ',')]
        names: Vec<String>,
        /// Port de l'API sur chaque machine (écoute sur 127.0.0.1), avec --hosts.
        #[arg(long, default_value_t = 8545)]
        api_port: u16,
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
        /// Adresse de l'API HTTP et de l'explorateur, par exemple 127.0.0.1:8545.
        #[arg(long)]
        api: Option<std::net::SocketAddr>,
    },
    /// Frappe des pièces en continu pour une adresse, via l'API d'un nœud.
    Mine {
        #[arg(long, default_value = "127.0.0.1:8545")]
        api: std::net::SocketAddr,
        /// Adresse qui reçoit la frappe (utu1…).
        #[arg(long)]
        address: String,
    },
    /// Envoie des pièces, via l'API d'un nœud.
    Send {
        #[arg(long, default_value = "127.0.0.1:8545")]
        api: std::net::SocketAddr,
        /// Fichier de clé de l'émetteur (mot de passe : UTUROA_KEY_PASSWORD).
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        to: String,
        /// Montant en pièces, par exemple 1,5.
        #[arg(long)]
        amount: String,
        #[arg(long)]
        memo: Option<String>,
    },
    /// Crée une clé chiffrée (portefeuille de test) et affiche son adresse.
    NewKey {
        #[arg(long)]
        out: PathBuf,
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
            mint_phase_blocks,
            base_port,
            hosts,
            names,
            api_port,
        } => {
            let validators = if hosts.is_empty() {
                validators
            } else {
                hosts.len()
            };
            if !names.is_empty() && names.len() != validators {
                bail!("--names doit donner un nom par validateur ({validators})");
            }
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
                let name = names
                    .get(i.saturating_sub(1))
                    .cloned()
                    .unwrap_or_else(|| format!("validateur-{i}"));
                entries.push(validator_entry(&kp.public_key(), &name));
            }
            let genesis = GenesisFile {
                chain_id,
                genesis_time_ms: now_ms(),
                mint_difficulty,
                validators: entries,
                block_interval_ms,
                mint_phase_blocks,
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
                if let Some(host) = hosts.get(i.saturating_sub(1)) {
                    addrs.push((base_port, format!("/ip4/{host}/tcp/{base_port}/p2p/{id}")));
                } else {
                    let offset = u16::try_from(i.saturating_sub(1)).unwrap_or(0);
                    let port = base_port.saturating_add(offset);
                    addrs.push((port, format!("/ip4/127.0.0.1/tcp/{port}/p2p/{id}")));
                }
            }
            if !hosts.is_empty() {
                // Un service systemd par machine, à copier dans /etc/systemd/system/.
                println!();
                for (i, host) in hosts.iter().enumerate() {
                    let n = i.saturating_add(1);
                    let peers: String = addrs
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, (_, a))| format!(" --peer {a}"))
                        .collect();
                    let name = names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("validateur-{n}"));
                    let unit = format!(
                        "[Unit]\nDescription=Validateur Uturoa {name}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nUser=uturoa\nWorkingDirectory=/opt/uturoa\nEnvironmentFile=/etc/uturoa/env\nExecStart=/opt/uturoa/uturoa-node run --genesis /opt/uturoa/genesis.json --key /opt/uturoa/validator.json --data-dir /var/lib/uturoa --listen /ip4/0.0.0.0/tcp/{base_port} --api 127.0.0.1:{api_port}{peers}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=multi-user.target\n"
                    );
                    let unit_path = dir.join(format!("uturoa-{n}.service"));
                    std::fs::write(&unit_path, unit)?;
                    println!(
                        "  {name} ({host}) : validator-{n}.json, d{n}/network.key, {}",
                        unit_path.display()
                    );
                }
                println!();
                println!("Voir docs/DEPLOYER-PROXMOX.md pour l'installation sur chaque machine.");
                return Ok(());
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
            api,
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
                        api,
                    },
                    Box::new(CachedPow::new(RandomXHasher::new(), 100_000)),
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
        Command::Mine { api, address } => {
            let beneficiary = address
                .parse()
                .map_err(|e| anyhow::anyhow!("adresse invalide : {e}"))?;
            println!("frappe pour {address} via {api} (Ctrl+C pour arrêter)");
            uturoa_node::client::mine(api, beneficiary, &RandomXHasher::new())?;
        }
        Command::Send {
            api,
            key,
            to,
            amount,
            memo,
        } => {
            let kp = KeyFile::load(&key)?.decrypt(&password(None)?)?;
            let to = to
                .parse()
                .map_err(|e| anyhow::anyhow!("adresse invalide : {e}"))?;
            let units = uturoa_node::client::parse_coins(&amount)?;
            let id = uturoa_node::client::send(api, &kp, to, units, memo)?;
            println!("transaction soumise : {id}");
        }
        Command::NewKey { out } => {
            let kp = KeyPair::generate();
            KeyFile::encrypt(&kp, &password(None)?, DEFAULT_KDF)?.save(&out)?;
            println!(
                "clé créée : {}\nadresse   : {}",
                out.display(),
                uturoa_core::Address::from_public_key(&kp.public_key())
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
