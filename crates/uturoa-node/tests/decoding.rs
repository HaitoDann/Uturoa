#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

//! Fuzzing rapide, rejoué à chaque modification : des milliers de mutations
//! aléatoires de messages réels (bloc, transaction, part de frappe, état
//! complet) ne font jamais planter le décodage, et seul l'encodage canonique
//! est accepté. Le fuzzing long (cargo-fuzz) tourne chaque nuit.

use borsh::{BorshDeserialize, BorshSerialize};
use uturoa_core::{
    Address, Block, BlockHeader, CommitCertificate, MintShare, Params, Transaction, TxBody,
    UNITS_PER_COIN, encode,
};
use uturoa_crypto::KeyPair;
use uturoa_node::network::{NetMessage, SyncRequest, SyncResponse};
use uturoa_state::State;

/// Générateur pseudo-aléatoire déterministe (xorshift64).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn canonical<T: BorshDeserialize + BorshSerialize>(bytes: &[u8]) {
    if let Ok(v) = borsh::from_slice::<T>(bytes) {
        assert_eq!(
            borsh::to_vec(&v).unwrap(),
            bytes,
            "encodage non canonique accepté"
        );
    }
}

fn check_all(bytes: &[u8]) {
    canonical::<NetMessage>(bytes);
    canonical::<SyncRequest>(bytes);
    canonical::<SyncResponse>(bytes);
    canonical::<Block>(bytes);
    canonical::<CommitCertificate>(bytes);
    canonical::<MintShare>(bytes);
    if let Ok(s) = uturoa_core::decode_canonical::<State>(bytes) {
        assert_eq!(borsh::to_vec(&s).unwrap(), bytes);
    }
}

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..=rng.below(4) {
        match rng.below(4) {
            0 if !b.is_empty() => {
                let i = rng.below(b.len());
                b[i] ^= 1 << rng.below(8);
            }
            1 if !b.is_empty() => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
            2 => {
                let i = rng.below(b.len() + 1);
                b.insert(i, rng.next() as u8);
            }
            _ if !b.is_empty() => {
                b.truncate(rng.below(b.len()));
            }
            _ => {}
        }
    }
    b
}

fn seeds() -> Vec<Vec<u8>> {
    let kp = KeyPair::generate();
    let tx = Transaction {
        chain_id: "utu-test".into(),
        sender: Address::from_public_key(&kp.public_key()),
        nonce: 1,
        fee: 1_000,
        body: TxBody::Transfer {
            to: Address::from_bytes([1; 20]),
            amount: UNITS_PER_COIN,
            memo: Some(b"facture".to_vec()),
        },
    }
    .sign(&kp, true);
    let share = MintShare {
        chain_id: "utu-test".into(),
        beneficiary: Address::from_bytes([2; 20]),
        anchor_height: 7,
        anchor_hash: [3; 32],
        nonce: 42,
    };
    let block = Block {
        header: BlockHeader {
            protocol_version: 1,
            chain_id: "utu-test".into(),
            height: 8,
            timestamp: 1_700_000_000_000,
            prev_hash: [4; 32],
            tx_root: [5; 32],
            last_commit_hash: [6; 32],
            state_root: [7; 32],
            validators_hash: [8; 32],
            proposer: 0,
        },
        last_commit: Some(CommitCertificate {
            height: 7,
            round: 0,
            block_hash: [9; 32],
            signatures: vec![(0, kp.sign(b"vote"))],
        }),
        txs: vec![tx.clone()],
        mint_shares: vec![share.clone()],
    };
    let mut state = State::genesis_with_validators(
        "utu-test",
        Params::default(),
        1,
        vec![(
            Address::from_public_key(&kp.public_key()),
            kp.public_key(),
            uturoa_core::ValidatorInfo {
                name: "v".into(),
                site: String::new(),
                contact: String::new(),
            },
        )],
    );
    state.credit_for_tests(&Address::from_bytes([1; 20]), UNITS_PER_COIN);
    vec![
        encode(&NetMessage::Txs(vec![tx])),
        encode(&NetMessage::Shares(vec![share])),
        encode(&block),
        encode(&SyncResponse::Blocks(vec![])),
        encode(&state),
    ]
}

/// Un état dont une table est encodée dans le désordre est refusé (cas
/// trouvé par le fuzzing).
#[test]
fn non_canonical_state_is_refused() {
    let b = include_bytes!("data/etat-non-canonique.bin");
    assert!(borsh::from_slice::<State>(b).is_ok());
    assert!(uturoa_core::decode_canonical::<State>(b).is_err());
}

#[test]
fn mutated_messages_never_crash_and_only_canonical_is_accepted() {
    let seeds = seeds();
    for s in &seeds {
        check_all(s);
    }
    let mut rng = Rng(0x5eed_0000_0000_0001);
    for _ in 0..20_000 {
        let seed = &seeds[rng.below(seeds.len())];
        check_all(&mutate(&mut rng, seed));
    }
}
