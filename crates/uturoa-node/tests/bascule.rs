#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Répétition complète, sur 8 nœuds réels : 4 validateurs d'amorçage, une
//! phase de frappe raccourcie (500 blocs), un mineur qui finance 4 nouveaux
//! validateurs, leur inscription avec 10 000 pièces de stake chacun, puis la
//! bascule : au bloc 501, ce sont eux qui valident, et la chaîne continue.
//!
//! Long (quelques minutes) : `cargo test -p uturoa-node --test bascule -- --ignored`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uturoa_core::{
    Address, Hash, MintShare, Payment, PublicKey, Transaction, TxBody, UNITS_PER_COIN,
    ValidatorInfo,
};
use uturoa_crypto::{Domain, KeyPair, hash_tagged_parts};
use uturoa_node::genesis_file::{GenesisFile, validator_entry};
use uturoa_node::mempool::seed_for;
use uturoa_node::network::NetMessage;
use uturoa_node::node::{SharedRef, lock};
use uturoa_node::service::{NodeConfig, RunningNode, launch, peer_id};
use uturoa_state::PowHasher;
use uturoa_state::staking::MIN_VALIDATOR_STAKE;

const PHASE: u64 = 500;
const CHAIN: &str = "utu-bascule";

struct FakePow;
impl PowHasher for FakePow {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        hash_tagged_parts(Domain::MintShare, &[seed, input])
    }
}

fn height(n: &RunningNode) -> u64 {
    lock(&n.shared).chain.height()
}

async fn wait_until(what: &str, limit: Duration, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < limit, "délai dépassé : {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Signe et soumet une transaction à un nœud (mempool et diffusion).
fn submit(shared: &SharedRef, key: &KeyPair, nonce: u64, body: TxBody) {
    let mut guard = lock(shared);
    let s = &mut *guard;
    let sender = Address::from_public_key(&key.public_key());
    let include_key = s
        .chain
        .state()
        .account(&sender)
        .is_none_or(|a| a.key.is_none())
        && nonce == 1;
    let mut tx = Transaction {
        chain_id: CHAIN.into(),
        sender,
        nonce,
        fee: 0,
        body,
    };
    tx.fee = s
        .chain
        .state()
        .required_fee(&tx.clone().sign(key, include_key))
        * 3;
    let stx = tx.sign(key, include_key);
    s.mempool.add(s.chain.state(), stx.clone()).unwrap();
    s.broadcast(NetMessage::Txs(vec![stx]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "long : répétition complète de la bascule"]
async fn mint_fund_register_and_switch_to_proof_of_stake() {
    let dir = tempfile::tempdir().unwrap();
    let boot: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let fresh: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let all: Vec<KeyPair> = boot.iter().chain(fresh.iter()).cloned().collect();
    let genesis = GenesisFile {
        chain_id: CHAIN.into(),
        genesis_time_ms: uturoa_node::node::now_ms(),
        mint_difficulty: 1,
        validators: boot
            .iter()
            .enumerate()
            .map(|(i, k)| validator_entry(&k.public_key(), &format!("amorce-{i}")))
            .collect(),
        block_interval_ms: Some(200),
        mint_phase_blocks: Some(PHASE),
    };
    let ports: Vec<u16> = (0..8)
        .map(|_| {
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        })
        .collect();
    let dirs: Vec<PathBuf> = (0..8).map(|i| dir.path().join(format!("d{i}"))).collect();
    let addrs: Vec<String> = (0..8)
        .map(|i| {
            format!(
                "/ip4/127.0.0.1/tcp/{}/p2p/{}",
                ports[i],
                peer_id(&dirs[i]).unwrap()
            )
        })
        .collect();
    let start = |i: usize| -> RunningNode {
        launch(
            NodeConfig {
                genesis: genesis.clone(),
                data_dir: dirs[i].clone(),
                key: Some(all[i].clone()),
                listen: format!("/ip4/127.0.0.1/tcp/{}", ports[i]).parse().unwrap(),
                peers: addrs
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
    };
    let mut nodes: Vec<RunningNode> = (0..4).map(start).collect();
    let n0 = nodes[0].shared.clone();

    // Un mineur soumet une part par bloc au nœud 0.
    let miner = KeyPair::generate();
    let miner_addr = Address::from_public_key(&miner.public_key());
    let mining = {
        let shared = n0.clone();
        tokio::spawn(async move {
            let mut last = u64::MAX;
            let mut nonce = 0u64;
            loop {
                {
                    let mut guard = lock(&shared);
                    let s = &mut *guard;
                    let h = s.chain.height();
                    if h != last && h < PHASE {
                        last = h;
                        let tip = s.chain.tip_hash();
                        if seed_for(s.chain.state(), h + 1, &tip).is_some() {
                            nonce += 1;
                            let share = MintShare {
                                chain_id: CHAIN.into(),
                                beneficiary: miner_addr,
                                anchor_height: h,
                                anchor_hash: tip,
                                nonce,
                            };
                            if s.mempool
                                .add_share(s.chain.state(), &tip, share.clone(), &FakePow)
                                .is_ok()
                            {
                                s.broadcast(NetMessage::Shares(vec![share]));
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };

    // Assez de pièces pour 4 validateurs (10 000 pièces chacun, plus les frais).
    let per_validator = MIN_VALIDATOR_STAKE + 10 * UNITS_PER_COIN;
    wait_until("frappe suffisante", Duration::from_secs(300), || {
        lock(&n0).chain.state().balance(&miner_addr) >= 4 * per_validator + UNITS_PER_COIN
    })
    .await;
    assert!(
        height(&nodes[0]) < PHASE - 30,
        "frappe trop lente pour la répétition"
    );

    // Le mineur finance les 4 futurs validateurs.
    let payments = fresh
        .iter()
        .map(|k| Payment {
            to: Address::from_public_key(&k.public_key()),
            amount: per_validator,
            memo: None,
        })
        .collect();
    submit(&n0, &miner, 1, TxBody::BatchTransfer { payments });
    wait_until("financement inclus", Duration::from_secs(60), || {
        let s = lock(&n0);
        fresh.iter().all(|k| {
            s.chain
                .state()
                .balance(&Address::from_public_key(&k.public_key()))
                == per_validator
        })
    })
    .await;

    // Inscription et stake propre, puis démarrage de leurs nœuds.
    for (i, k) in fresh.iter().enumerate() {
        submit(
            &n0,
            k,
            1,
            TxBody::RegisterValidator {
                key: k.public_key(),
                info: ValidatorInfo {
                    name: format!("nouveau-{i}"),
                    site: "Tahaa".into(),
                    contact: String::new(),
                },
                commission_bps: 500,
                max_commission_bps: 2_000,
            },
        );
        submit(
            &n0,
            k,
            2,
            TxBody::Delegate {
                validator: k.public_key(),
                amount: MIN_VALIDATOR_STAKE,
            },
        );
    }
    for i in 4..8 {
        nodes.push(start(i));
    }
    wait_until("stake inscrit", Duration::from_secs(60), || {
        let s = lock(&n0);
        fresh.iter().all(|k| {
            let op = Address::from_public_key(&k.public_key());
            s.chain
                .state()
                .staking()
                .validator(&op)
                .is_some_and(|v| v.self_stake == MIN_VALIDATOR_STAKE)
        })
    })
    .await;
    assert!(height(&nodes[0]) < PHASE - 2, "inscription trop tardive");

    // Bascule : au-delà de la phase, l'ensemble est celui des validateurs à stake.
    wait_until(
        "bascule et 10 blocs de plus",
        Duration::from_secs(300),
        || nodes.iter().all(|n| height(n) >= PHASE + 10),
    )
    .await;
    let expected: BTreeSet<PublicKey> = fresh.iter().map(KeyPair::public_key).collect();
    for n in &nodes {
        let s = lock(&n.shared);
        let set: BTreeSet<PublicKey> = s.chain.next_validators().into_iter().collect();
        assert_eq!(set, expected, "ensemble inattendu après la bascule");
        // Le dernier bloc a bien été signé par les nouveaux validateurs.
        let h = s.chain.height();
        let commit = s.chain.store().commit(h).unwrap().unwrap();
        assert!(commit.signatures.len() >= 3);
    }
    // Plus de frappe après la bascule.
    assert_eq!(lock(&n0).chain.state().mint().pot(), 0);

    mining.abort();
    for n in nodes {
        let _ = n.shutdown.send(true);
    }
}
