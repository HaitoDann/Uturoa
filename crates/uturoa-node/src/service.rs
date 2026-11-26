//! Démarrage d'un nœud complet : chaîne, réseau et consensus.

use crate::chain::Chain;
use crate::genesis_file::GenesisFile;
use crate::network::{self, NetConfig};
use crate::node::{self, Shared, SharedRef, lock};
use anyhow::Result;
use libp2p::{Multiaddr, PeerId};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use uturoa_core::Hash;
use uturoa_crypto::KeyPair;
use uturoa_state::PowHasher;

pub struct NodeConfig {
    pub genesis: GenesisFile,
    pub data_dir: PathBuf,
    /// Clé de validateur ; absente pour un observateur.
    pub key: Option<KeyPair>,
    pub listen: Multiaddr,
    pub peers: Vec<Multiaddr>,
    pub trusted_block: Option<Hash>,
    /// Adresse d'écoute de l'API HTTP, par exemple `127.0.0.1:8545`.
    pub api: Option<std::net::SocketAddr>,
}

pub struct RunningNode {
    pub shared: SharedRef,
    pub peer_id: PeerId,
    pub shutdown: watch::Sender<bool>,
    pub tasks: Vec<JoinHandle<Result<()>>>,
}

/// Identifiant réseau du nœud qui utilisera ce répertoire.
pub fn peer_id(data_dir: &std::path::Path) -> Result<PeerId> {
    Ok(network::load_or_create_identity(data_dir)?
        .public()
        .to_peer_id())
}

/// Ouvre la chaîne et lance le réseau et le consensus.
pub fn launch(config: NodeConfig, pow: Box<dyn PowHasher + Send>) -> Result<RunningNode> {
    let identity = network::load_or_create_identity(&config.data_dir)?;
    let peer_id = identity.public().to_peer_id();
    let chain = Chain::open(&config.data_dir, &config.genesis, pow)?;
    let shared: SharedRef = Arc::new(Mutex::new(Shared::new(chain)?));
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (req_tx, req_rx) = mpsc::unbounded_channel();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = watch::channel(false);
    {
        let mut s = lock(&shared);
        s.outbound = Some(out_tx);
        s.requests = Some(req_tx);
        s.trusted_block = config.trusted_block;
    }
    let net = tokio::spawn(network::run(
        identity,
        NetConfig {
            listen: config.listen,
            peers: config.peers,
        },
        shared.clone(),
        out_rx,
        req_rx,
        in_tx,
        stop_rx.clone(),
    ));
    let consensus = tokio::spawn(node::run(
        shared.clone(),
        config.key,
        in_rx,
        stop_rx.clone(),
    ));
    let mut tasks = vec![net, consensus];
    if let Some(addr) = config.api {
        let app = crate::api::router(shared.clone());
        let mut stop = stop_rx;
        tasks.push(tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!(%addr, "API démarrée");
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop.changed().await;
                })
                .await?;
            anyhow::Ok(())
        }));
    }
    Ok(RunningNode {
        shared,
        peer_id,
        shutdown: stop_tx,
        tasks,
    })
}
