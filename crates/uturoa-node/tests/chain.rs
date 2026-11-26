#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Chaîne locale : construction, vérification, stockage et redémarrage.

use uturoa_core::{
    Address, Block, CommitCertificate, Hash, MIN_RESERVE, MintShare, Transaction, TxBody,
    UNITS_PER_COIN, Vote, VoteKind,
};
use uturoa_crypto::{Domain, KeyPair, hash_tagged_parts};
use uturoa_node::chain::Chain;
use uturoa_node::genesis_file::{GenesisFile, validator_entry};
use uturoa_state::{PowHasher, meets_target};

/// Preuve de travail factice, pour des tests rapides.
struct FakePow;
impl PowHasher for FakePow {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        hash_tagged_parts(Domain::MintShare, &[seed, input])
    }
}

fn setup(n: usize) -> (GenesisFile, Vec<KeyPair>) {
    let kps: Vec<KeyPair> = (0..n).map(|_| KeyPair::generate()).collect();
    let genesis = GenesisFile {
        chain_id: "utu-test".into(),
        genesis_time_ms: 1_000,
        mint_difficulty: 1,
        validators: kps
            .iter()
            .enumerate()
            .map(|(i, k)| validator_entry(&k.public_key(), &format!("v{i}")))
            .collect(),
        block_interval_ms: None,
    };
    (genesis, kps)
}

/// Certificat signé par tous les validateurs du prochain bloc.
fn certify(chain: &Chain, block: &Block, kps: &[KeyPair]) -> CommitCertificate {
    let vote = Vote {
        chain_id: "utu-test".into(),
        kind: VoteKind::Precommit,
        height: block.header.height,
        round: 0,
        block_hash: Some(block.hash()),
    };
    let set = chain.next_validators();
    let signatures = set
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let kp = kps.iter().find(|k| k.public_key() == *key).unwrap();
            (i as u16, vote.sign(kp))
        })
        .collect();
    CommitCertificate {
        height: block.header.height,
        round: 0,
        block_hash: block.hash(),
        signatures,
    }
}

fn advance(chain: &mut Chain, kps: &[KeyPair], blocks: u64, miner: Address) {
    for _ in 0..blocks {
        let h = chain.height() + 1;
        // Une part de frappe par bloc, ancrée sur le bloc précédent.
        let shares = if h >= 2 {
            let mut share = MintShare {
                chain_id: "utu-test".into(),
                beneficiary: miner,
                anchor_height: h - 1,
                anchor_hash: chain.tip_hash(),
                nonce: 0,
            };
            let seed = chain.state().mint().seed_for(h).unwrap();
            while !meets_target(&FakePow.hash(&seed, &share.pow_input()), 1) {
                share.nonce += 1;
            }
            vec![share]
        } else {
            vec![]
        };
        let block = chain
            .build_block(0, vec![], shares, 1_000 + h * 12_000)
            .unwrap();
        let commit = certify(chain, &block, kps);
        chain.commit_block(block, commit).unwrap();
    }
}

#[test]
fn blocks_are_built_verified_committed_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (genesis, kps) = setup(4);
    let miner = Address::from_bytes([7; 20]);
    let root = {
        let mut chain = Chain::open(dir.path(), &genesis, Box::new(FakePow)).unwrap();
        advance(&mut chain, &kps, 120, miner);
        assert_eq!(chain.height(), 120);
        // Deux périodes de frappe partagées au mineur.
        assert!(chain.state().balance(&miner) > 0);
        chain.state().state_root()
    };
    // Redémarrage sans instantané : tout est rejoué depuis la genèse.
    let chain = Chain::open(dir.path(), &genesis, Box::new(FakePow)).unwrap();
    assert_eq!(chain.height(), 120);
    assert_eq!(chain.state().state_root(), root);
}

#[test]
fn restart_from_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let (genesis, kps) = setup(4);
    let miner = Address::from_bytes([7; 20]);
    let root = {
        let mut chain = Chain::open(dir.path(), &genesis, Box::new(FakePow)).unwrap();
        chain.set_snapshot_interval(25);
        advance(&mut chain, &kps, 60, miner);
        chain.state().state_root()
    };
    let chain = Chain::open(dir.path(), &genesis, Box::new(FakePow)).unwrap();
    assert!(chain.store().latest_snapshot().unwrap().unwrap().height == 50);
    assert_eq!(chain.height(), 60);
    assert_eq!(chain.state().state_root(), root);
}

#[test]
fn tampered_blocks_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (genesis, kps) = setup(4);
    let mut chain = Chain::open(dir.path(), &genesis, Box::new(FakePow)).unwrap();
    advance(&mut chain, &kps, 3, Address::from_bytes([7; 20]));
    let good = chain.build_block(0, vec![], vec![], 100_000).unwrap();

    let mut bad_root = good.clone();
    bad_root.header.state_root = [9; 32];
    assert!(chain.verify(&bad_root, u64::MAX).is_err());

    let mut bad_prev = good.clone();
    bad_prev.header.prev_hash = [9; 32];
    assert!(chain.verify(&bad_prev, u64::MAX).is_err());

    let mut no_commit = good.clone();
    no_commit.last_commit = None;
    no_commit.header.last_commit_hash = no_commit.compute_last_commit_hash();
    assert!(chain.verify(&no_commit, u64::MAX).is_err());

    // Transaction ajoutée sans mettre à jour la racine.
    let kp = KeyPair::generate();
    let mut extra = good.clone();
    extra.txs.push(
        Transaction {
            chain_id: "utu-test".into(),
            sender: Address::from_public_key(&kp.public_key()),
            nonce: 1,
            fee: UNITS_PER_COIN,
            body: TxBody::Transfer {
                to: Address::from_bytes([1; 20]),
                amount: MIN_RESERVE,
                memo: None,
            },
        }
        .sign(&kp, true),
    );
    assert!(chain.verify(&extra, u64::MAX).is_err());

    // Horodatage trop loin dans le futur.
    assert!(chain.verify(&good, 1_000).is_err());
    // Le bon bloc passe, mais pas avec un certificat sans quorum.
    assert!(chain.verify(&good, u64::MAX).is_ok());
    let mut weak = certify(&chain, &good, &kps);
    weak.signatures.truncate(2);
    assert!(chain.commit_block(good, weak).is_err());
}

#[test]
fn snapshot_is_installed_by_a_fresh_node_and_tampering_is_refused() {
    let (genesis, kps) = setup(4);
    let miner = Address::from_bytes([7; 20]);
    let src = tempfile::tempdir().unwrap();
    let mut a = Chain::open(src.path(), &genesis, Box::new(FakePow)).unwrap();
    a.set_snapshot_interval(30);
    advance(&mut a, &kps, 65, miner);
    let (block, commit, bytes) = a.snapshot().unwrap().unwrap();
    assert_eq!(block.header.height, 60);

    // État falsifié : refusé (la racine ne correspond plus).
    let dst = tempfile::tempdir().unwrap();
    let mut b = Chain::open(dst.path(), &genesis, Box::new(FakePow)).unwrap();
    let mut forged: uturoa_state::State = uturoa_core::decode(&bytes).unwrap();
    forged.credit_for_tests(&Address::from_bytes([66; 20]), UNITS_PER_COIN);
    let forged = uturoa_core::encode(&forged);
    assert!(
        b.install_snapshot(block.clone(), commit.clone(), &forged, None)
            .is_err()
    );
    // Certificat sans quorum : refusé.
    let mut weak = commit.clone();
    weak.signatures.truncate(2);
    assert!(
        b.install_snapshot(block.clone(), weak, &bytes, None)
            .is_err()
    );
    // Le vrai instantané : installé, puis la chaîne repart de là.
    b.install_snapshot(block, commit, &bytes, None).unwrap();
    assert_eq!(b.height(), 60);
    for h in 61..=65 {
        let blk = a.store().block(h).unwrap().unwrap();
        let c = a.store().commit(h).unwrap().unwrap();
        b.commit_block(blk, c).unwrap();
    }
    assert_eq!(b.state().state_root(), a.state().state_root());
    // Et après redémarrage.
    drop(b);
    let b = Chain::open(dst.path(), &genesis, Box::new(FakePow)).unwrap();
    assert_eq!(b.height(), 65);
}
