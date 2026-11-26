#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Réseau réel en local : 4 validateurs reliés par libp2p produisent des
//! blocs, un mineur frappe, puis dépense ce qu'il a frappé.

use std::time::{Duration, Instant};
use uturoa_core::{Address, Hash, MIN_RESERVE, MintShare, Transaction, TxBody};
use uturoa_crypto::{Domain, KeyPair, hash_tagged_parts};
use uturoa_node::genesis_file::{GenesisFile, validator_entry};
use uturoa_node::network::NetMessage;
use uturoa_node::node::lock;
use uturoa_node::service::{NodeConfig, RunningNode, launch, peer_id};
use uturoa_state::mint::MINT_PERIOD;
use uturoa_state::{PowHasher, meets_target};

struct FakePow;
impl PowHasher for FakePow {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        hash_tagged_parts(Domain::MintShare, &[seed, input])
    }
}

fn height(node: &RunningNode) -> u64 {
    lock(&node.shared).chain.height()
}

async fn wait_for(nodes: &[RunningNode], target: u64, limit: Duration) {
    let start = Instant::now();
    while nodes.iter().any(|n| height(n) < target) {
        assert!(
            start.elapsed() < limit,
            "hauteurs bloquées : {:?} ; pairs connectés : {:?} ; tâches arrêtées : {:?}",
            nodes.iter().map(height).collect::<Vec<_>>(),
            nodes
                .iter()
                .map(|n| lock(&n.shared).peers)
                .collect::<Vec<_>>(),
            nodes
                .iter()
                .map(|n| n.tasks.iter().filter(|t| t.is_finished()).count())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn four_validators_produce_blocks_mint_and_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let kps: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let genesis = GenesisFile {
        chain_id: "utu-net".into(),
        genesis_time_ms: uturoa_node::node::now_ms(),
        mint_difficulty: 1,
        validators: kps
            .iter()
            .enumerate()
            .map(|(i, k)| validator_entry(&k.public_key(), &format!("v{i}")))
            .collect(),
        block_interval_ms: Some(200),
    };
    // Ports réellement libres, choisis par le système : des ports fixes
    // peuvent tomber dans une plage réservée (fréquent sous Windows).
    let ports: Vec<u16> = (0..5)
        .map(|_| {
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        })
        .collect();
    let dirs: Vec<_> = (0..4).map(|i| dir.path().join(format!("d{i}"))).collect();
    let addrs: Vec<String> = dirs
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
    let nodes: Vec<RunningNode> = (0..4)
        .map(|i| {
            launch(
                NodeConfig {
                    genesis: genesis.clone(),
                    data_dir: dirs[i].clone(),
                    key: Some(kps[i].clone()),
                    listen: format!("/ip4/127.0.0.1/tcp/{}", ports[i]).parse().unwrap(),
                    peers: addrs
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, a)| a.parse().unwrap())
                        .collect(),
                    trusted_block: None,
                    api: (i == 0).then(|| format!("127.0.0.1:{}", ports[4]).parse().unwrap()),
                },
                Box::new(FakePow),
            )
            .unwrap()
        })
        .collect();

    // La chaîne avance sur les 4 nœuds.
    wait_for(&nodes, 5, Duration::from_secs(60)).await;

    // Un mineur soumet une part au nœud 0, qui la diffuse.
    let miner = KeyPair::generate();
    let miner_addr = Address::from_public_key(&miner.public_key());
    let share = {
        let s = lock(&nodes[0].shared);
        let h = s.chain.height() + 1;
        let mut share = MintShare {
            chain_id: "utu-net".into(),
            beneficiary: miner_addr,
            anchor_height: h - 1,
            anchor_hash: s.chain.tip_hash(),
            nonce: 0,
        };
        let seed = s.chain.state().mint().seed_for(h).unwrap();
        while !meets_target(&FakePow.hash(&seed, &share.pow_input()), 1) {
            share.nonce += 1;
        }
        share
    };
    {
        let mut guard = lock(&nodes[0].shared);
        let s = &mut *guard;
        let tip = s.chain.tip_hash();
        s.mempool
            .add_share(s.chain.state(), &tip, share.clone(), &FakePow)
            .unwrap();
        s.broadcast(NetMessage::Shares(vec![share]));
    }
    // Fin de la période de frappe : le mineur est payé partout.
    wait_for(&nodes, MINT_PERIOD + 2, Duration::from_secs(120)).await;
    let paid = lock(&nodes[3].shared).chain.state().balance(&miner_addr);
    assert!(paid > 0, "le mineur n'a rien reçu");

    // Le mineur dépense une partie de sa frappe, via le nœud 2.
    let to = Address::from_bytes([5; 20]);
    {
        let mut guard = lock(&nodes[2].shared);
        let s = &mut *guard;
        let mut tx = Transaction {
            chain_id: "utu-net".into(),
            sender: miner_addr,
            nonce: 1,
            fee: 0,
            body: TxBody::Transfer {
                to,
                amount: MIN_RESERVE,
                memo: Some(b"premier envoi".to_vec()),
            },
        };
        tx.fee = s.chain.state().required_fee(&tx.clone().sign(&miner, true)) * 2;
        let stx = tx.sign(&miner, true);
        s.mempool.add(s.chain.state(), stx.clone()).unwrap();
        s.broadcast(NetMessage::Txs(vec![stx]));
    }
    let start = Instant::now();
    while lock(&nodes[1].shared).chain.state().balance(&to) == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "envoi jamais inclus"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Tous les nœuds ont la même racine d'état à une même hauteur.
    let h = nodes.iter().map(height).min().unwrap();
    wait_for(&nodes, h + 1, Duration::from_secs(30)).await;
    let roots: Vec<Hash> = nodes
        .iter()
        .map(|n| {
            let s = lock(&n.shared);
            s.chain.store().block(h).unwrap().unwrap().header.state_root
        })
        .collect();
    assert!(roots.windows(2).all(|w| w[0] == w[1]));

    for n in nodes {
        let _ = n.shutdown.send(true);
    }
}
