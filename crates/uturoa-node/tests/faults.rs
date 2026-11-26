#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Exercices de panne, rejoués automatiquement sur 4 validateurs locaux :
//! un validateur arrêté (la chaîne continue), deux arrêtés (la chaîne
//! s'arrête sans erreur), puis redémarrage (reprise et rattrapage, sans
//! double signature).

use std::path::PathBuf;
use std::time::{Duration, Instant};
use uturoa_core::Hash;
use uturoa_crypto::{Domain, KeyPair, hash_tagged_parts};
use uturoa_node::genesis_file::{GenesisFile, validator_entry};
use uturoa_node::node::lock;
use uturoa_node::service::{NodeConfig, RunningNode, launch, peer_id};
use uturoa_state::PowHasher;

struct FakePow;
impl PowHasher for FakePow {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        hash_tagged_parts(Domain::MintShare, &[seed, input])
    }
}

struct Net {
    genesis: GenesisFile,
    kps: Vec<KeyPair>,
    dirs: Vec<PathBuf>,
    ports: Vec<u16>,
    addrs: Vec<String>,
}

impl Net {
    fn start(&self, i: usize) -> RunningNode {
        launch(
            NodeConfig {
                genesis: self.genesis.clone(),
                data_dir: self.dirs[i].clone(),
                key: Some(self.kps[i].clone()),
                listen: format!("/ip4/127.0.0.1/tcp/{}", self.ports[i])
                    .parse()
                    .unwrap(),
                peers: self
                    .addrs
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, a)| a.parse().unwrap())
                    .collect(),
                trusted_block: None,
                api: None,
            },
            Box::new(FakePow),
        )
        .unwrap()
    }
}

async fn stop(node: RunningNode) {
    let _ = node.shutdown.send(true);
    for t in node.tasks {
        let _ = t.await;
    }
}

fn height(node: &RunningNode) -> u64 {
    lock(&node.shared).chain.height()
}

async fn wait_for(nodes: &[&RunningNode], target: u64, limit: Duration) {
    let start = Instant::now();
    while nodes.iter().any(|n| height(n) < target) {
        assert!(
            start.elapsed() < limit,
            "hauteurs bloquées sous {target} : {:?} ; pairs connectés : {:?}",
            nodes.iter().map(|n| height(n)).collect::<Vec<_>>(),
            nodes
                .iter()
                .map(|n| lock(&n.shared).peers)
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chain_survives_one_halts_safely_with_two_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let kps: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let genesis = GenesisFile {
        chain_id: "utu-pannes".into(),
        genesis_time_ms: uturoa_node::node::now_ms(),
        mint_difficulty: 1,
        validators: kps
            .iter()
            .enumerate()
            .map(|(i, k)| validator_entry(&k.public_key(), &format!("v{i}")))
            .collect(),
        block_interval_ms: Some(200),
        mint_phase_blocks: None,
    };
    let ports: Vec<u16> = (0..4)
        .map(|_| {
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        })
        .collect();
    let dirs: Vec<PathBuf> = (0..4).map(|i| dir.path().join(format!("d{i}"))).collect();
    let addrs = dirs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            format!(
                "/ip4/127.0.0.1/tcp/{}/p2p/{}",
                ports[i],
                peer_id(d).unwrap()
            )
        })
        .collect();
    let net = Net {
        genesis,
        kps,
        dirs,
        ports,
        addrs,
    };
    let mut nodes: Vec<Option<RunningNode>> = (0..4).map(|i| Some(net.start(i))).collect();
    let live = |nodes: &Vec<Option<RunningNode>>| -> Vec<u64> {
        nodes.iter().flatten().map(height).collect()
    };

    // Démarrage.
    {
        let all: Vec<&RunningNode> = nodes.iter().flatten().collect();
        wait_for(&all, 4, Duration::from_secs(60)).await;
    }

    // 1. Un validateur arrêté : la chaîne continue.
    stop(nodes[3].take().unwrap()).await;
    let h1 = live(&nodes).into_iter().max().unwrap();
    {
        let three: Vec<&RunningNode> = nodes.iter().flatten().collect();
        wait_for(&three, h1 + 4, Duration::from_secs(90)).await;
    }

    // 2. Deux validateurs arrêtés : la chaîne s'arrête.
    stop(nodes[2].take().unwrap()).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let frozen = live(&nodes);
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(live(&nodes), frozen, "la chaîne a avancé sans quorum");

    // 3. Redémarrage des deux : reprise, et rattrapage des absents.
    nodes[2] = Some(net.start(2));
    nodes[3] = Some(net.start(3));
    let target = frozen.iter().copied().max().unwrap() + 4;
    {
        let all: Vec<&RunningNode> = nodes.iter().flatten().collect();
        wait_for(&all, target, Duration::from_secs(120)).await;
    }

    // Mêmes blocs partout, et aucune preuve de double signature.
    let h = target - 1;
    let roots: Vec<Hash> = nodes
        .iter()
        .flatten()
        .map(|n| {
            lock(&n.shared)
                .chain
                .store()
                .block(h)
                .unwrap()
                .unwrap()
                .hash()
        })
        .collect();
    assert!(
        roots.windows(2).all(|w| w[0] == w[1]),
        "chaînes divergentes"
    );
    for n in nodes.iter().flatten() {
        let s = lock(&n.shared);
        for i in 1..=h {
            let block = s.chain.store().block(i).unwrap().unwrap();
            assert!(
                !block
                    .txs
                    .iter()
                    .any(|t| matches!(t.tx.body, uturoa_core::TxBody::ReportEquivocation { .. })),
                "preuve d'équivoque au bloc {i}"
            );
        }
    }

    for n in nodes.into_iter().flatten() {
        stop(n).await;
    }
}
