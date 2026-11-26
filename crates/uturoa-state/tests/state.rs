#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use uturoa_core::{
    Address, Authorization, MAX_SUPPLY, MIN_BASE_FEE_PER_BYTE, MIN_RESERVE, MINT_PER_BLOCK,
    MINT_PHASE_BLOCKS, Multisig, Params, Payment, SignedTransaction, Transaction, TxBody,
    UNITS_PER_COIN, encode, forge_amount,
};
use uturoa_crypto::KeyPair;
use uturoa_state::{BlockError, BlockInput, PowHasher, State, TxError, block_issuance};

const CHAIN: &str = "utu-test";

/// Preuve de travail factice et déterministe pour les tests.
struct FakePow;
impl PowHasher for FakePow {
    fn hash(&self, seed: &uturoa_core::Hash, input: &[u8]) -> uturoa_core::Hash {
        uturoa_crypto::hash_tagged_parts(uturoa_crypto::Domain::MintShare, &[seed, input])
    }
}

fn block<'a>(
    height: u64,
    txs: &'a [SignedTransaction],
    proposer: &'a Address,
    bytes: u64,
) -> BlockInput<'a> {
    BlockInput {
        height,
        parent_hash: [height as u8; 32],
        txs,
        shares: &[],
        proposer,
        last_commit: None,
        block_bytes: bytes,
    }
}

struct User {
    kp: KeyPair,
    addr: Address,
    nonce: u64,
}

impl User {
    fn new() -> Self {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        Self { kp, addr, nonce: 0 }
    }

    /// Transaction signée avec des frais suffisants pour l'état donné.
    fn tx(&mut self, state: &State, body: TxBody, include_key: bool) -> SignedTransaction {
        self.nonce += 1;
        let mut tx = Transaction {
            chain_id: CHAIN.into(),
            sender: self.addr,
            nonce: self.nonce,
            fee: 0,
            body,
        };
        // Les frais dépendent de la taille, qui dépend un peu des frais.
        for _ in 0..3 {
            let stx = tx.clone().sign(&self.kp, include_key);
            tx.fee = state.required_fee(&stx);
        }
        tx.sign(&self.kp, include_key)
    }
}

fn transfer(to: Address, amount: u64) -> TxBody {
    TxBody::Transfer {
        to,
        amount,
        memo: None,
    }
}

fn funded(user: &User, coins: u64) -> State {
    let mut s = State::genesis(CHAIN, Params::default());
    s.credit_for_tests(&user.addr, coins * UNITS_PER_COIN);
    s
}

#[test]
fn simple_transfer_moves_value_and_melts_fees() {
    let mut alice = User::new();
    let bob = User::new();
    let proposer = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 100);
    let supply_before = s.supply();
    let stx = alice.tx(&s, transfer(bob.addr, UNITS_PER_COIN), true);
    let fee = stx.tx.fee;
    s.apply_tx(&stx, &proposer).unwrap();

    assert_eq!(s.balance(&bob.addr), UNITS_PER_COIN);
    assert_eq!(s.balance(&alice.addr), 99 * UNITS_PER_COIN - fee);
    let to_proposer = fee - fee * 80 / 100;
    assert_eq!(s.balance(&proposer), to_proposer);
    assert_eq!(s.supply(), supply_before - fee * 80 / 100);
    assert!(s.check_invariant());
    // Un envoi simple coûte environ 0,0001 pièce au plancher.
    assert!(
        fee > UNITS_PER_COIN / 20_000 && fee < UNITS_PER_COIN / 5_000,
        "{fee}"
    );
}

#[test]
fn first_spend_needs_key_then_forbids_it() {
    let mut alice = User::new();
    let bob = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);

    let no_key = alice.tx(&s, transfer(bob.addr, MIN_RESERVE), false);
    assert!(matches!(
        s.apply_tx(&no_key, &p),
        Err(TxError::Unauthorized(_))
    ));
    alice.nonce -= 1;
    s.apply_tx(&alice.tx(&s, transfer(bob.addr, MIN_RESERVE), true), &p)
        .unwrap();
    let again_key = alice.tx(&s, transfer(bob.addr, MIN_RESERVE), true);
    assert!(matches!(
        s.apply_tx(&again_key, &p),
        Err(TxError::Unauthorized(_))
    ));
    alice.nonce -= 1;
    s.apply_tx(&alice.tx(&s, transfer(bob.addr, MIN_RESERVE), false), &p)
        .unwrap();
}

#[test]
fn key_must_match_address() {
    let mut alice = User::new();
    let mallory = KeyPair::generate();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);
    let mut tx = alice
        .tx(
            &s,
            transfer(Address::from_bytes([1; 20]), MIN_RESERVE),
            true,
        )
        .tx;
    tx.fee *= 2;
    let forged = tx.sign(&mallory, true);
    assert!(matches!(
        s.apply_tx(&forged, &p),
        Err(TxError::Unauthorized(_))
    ));
}

#[test]
fn nonce_replay_and_wrong_chain_are_rejected() {
    let mut alice = User::new();
    let bob = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);
    let stx = alice.tx(&s, transfer(bob.addr, MIN_RESERVE), true);
    s.apply_tx(&stx, &p).unwrap();
    assert!(matches!(
        s.apply_tx(&stx, &p),
        Err(TxError::Unauthorized(_)) | Err(TxError::BadNonce { .. })
    ));

    let mut other = alice.tx(&s, transfer(bob.addr, MIN_RESERVE), false).tx;
    other.chain_id = "autre".into();
    let other = other.sign(&alice.kp, false);
    assert_eq!(s.apply_tx(&other, &p), Err(TxError::WrongChain));
}

#[test]
fn fees_below_base_fee_are_rejected() {
    let mut alice = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);
    let mut tx = alice
        .tx(
            &s,
            transfer(Address::from_bytes([1; 20]), MIN_RESERVE),
            true,
        )
        .tx;
    tx.fee = 1;
    let stx = tx.sign(&alice.kp, true);
    assert!(matches!(
        s.apply_tx(&stx, &p),
        Err(TxError::FeeTooLow { .. })
    ));
}

#[test]
fn reserve_rules() {
    let mut alice = User::new();
    let bob = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 1);

    // Un nouveau compte doit recevoir au moins 0,01 pièce.
    let small = alice.tx(&s, transfer(bob.addr, MIN_RESERVE - 1), true);
    assert_eq!(s.apply_tx(&small, &p), Err(TxError::NewAccountBelowReserve));
    alice.nonce -= 1;

    // Le solde restant ne peut pas tomber entre 0 et la réserve.
    let fee = alice.tx(&s, transfer(bob.addr, MIN_RESERVE), true).tx.fee;
    alice.nonce -= 1;
    let leave_dust = UNITS_PER_COIN - fee - 1;
    let dust = alice.tx(&s, transfer(bob.addr, leave_dust), true);
    assert_eq!(s.apply_tx(&dust, &p), Err(TxError::BelowReserve));
    alice.nonce -= 1;

    // Tout envoyer est permis : le compte reste, à zéro, avec son nonce.
    let all = alice.tx(&s, transfer(bob.addr, UNITS_PER_COIN - fee), true);
    s.apply_tx(&all, &p).unwrap();
    assert_eq!(s.balance(&alice.addr), 0);
    assert_eq!(s.account(&alice.addr).unwrap().nonce, 1);

    // Plus de solde : impossible de payer les frais.
    let broke = alice.tx(&s, transfer(bob.addr, MIN_RESERVE), false);
    assert_eq!(s.apply_tx(&broke, &p), Err(TxError::InsufficientBalance));
}

#[test]
fn malformed_transfers() {
    let mut alice = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);
    let to_self = alice.tx(&s, transfer(alice.addr, MIN_RESERVE), true);
    assert!(matches!(
        s.apply_tx(&to_self, &p),
        Err(TxError::Malformed(_))
    ));
    alice.nonce -= 1;
    let zero = alice.tx(&s, transfer(Address::from_bytes([1; 20]), 0), true);
    assert!(matches!(s.apply_tx(&zero, &p), Err(TxError::Malformed(_))));
    alice.nonce -= 1;
    let long_memo = TxBody::Transfer {
        to: Address::from_bytes([1; 20]),
        amount: MIN_RESERVE,
        memo: Some(vec![b'x'; 65]),
    };
    let memo = alice.tx(&s, long_memo, true);
    assert!(matches!(s.apply_tx(&memo, &p), Err(TxError::Malformed(_))));
}

#[test]
fn batch_transfer() {
    let mut alice = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);
    let payments: Vec<Payment> = (1..=5u8)
        .map(|i| Payment {
            to: Address::from_bytes([i; 20]),
            amount: UNITS_PER_COIN,
            memo: Some(vec![i]),
        })
        .collect();
    let stx = alice.tx(&s, TxBody::BatchTransfer { payments }, true);
    let fee = stx.tx.fee;
    s.apply_tx(&stx, &p).unwrap();
    for i in 1..=5u8 {
        assert_eq!(s.balance(&Address::from_bytes([i; 20])), UNITS_PER_COIN);
    }
    assert_eq!(s.balance(&alice.addr), 5 * UNITS_PER_COIN - fee);

    let empty = alice.tx(&s, TxBody::BatchTransfer { payments: vec![] }, false);
    assert!(matches!(s.apply_tx(&empty, &p), Err(TxError::Malformed(_))));
}

#[test]
fn multisig_two_of_three() {
    let mut owner = User::new();
    let signers: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
    let ms = Multisig {
        signers: signers.iter().map(KeyPair::public_key).collect(),
        threshold: 2,
    };
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&owner, 10);
    let conf = owner.tx(&s, TxBody::ConfigureMultisig { multisig: Some(ms) }, true);
    s.apply_tx(&conf, &p).unwrap();

    // La clé seule ne suffit plus.
    let single = owner.tx(
        &s,
        transfer(Address::from_bytes([1; 20]), MIN_RESERVE),
        false,
    );
    assert!(matches!(
        s.apply_tx(&single, &p),
        Err(TxError::Unauthorized(_))
    ));
    owner.nonce -= 1;

    let mut tx = Transaction {
        chain_id: CHAIN.into(),
        sender: owner.addr,
        nonce: 2,
        fee: 0,
        body: transfer(Address::from_bytes([1; 20]), MIN_RESERVE),
    };
    let build = |tx: &Transaction, idx: &[u8]| SignedTransaction {
        auth: Authorization::Multi {
            approvals: idx
                .iter()
                .map(|&i| tx.approve(i, &signers[usize::from(i)]))
                .collect(),
        },
        tx: tx.clone(),
    };
    tx.fee = s.required_fee(&build(&tx, &[0, 2])) * 2;
    assert!(matches!(
        s.apply_tx(&build(&tx, &[1]), &p),
        Err(TxError::Unauthorized(_))
    ));
    s.apply_tx(&build(&tx, &[0, 2]), &p).unwrap();
    assert_eq!(s.balance(&Address::from_bytes([1; 20])), MIN_RESERVE);

    // Règle invalide refusée.
    let bad = Multisig {
        signers: vec![],
        threshold: 1,
    };
    let mut t3 = tx.clone();
    t3.nonce = 3;
    t3.body = TxBody::ConfigureMultisig {
        multisig: Some(bad),
    };
    assert!(matches!(
        s.apply_tx(&build(&t3, &[0, 1]), &p),
        Err(TxError::Malformed(_))
    ));
}

#[test]
fn blocks_are_atomic_and_issue_units() {
    let mut alice = User::new();
    let bob = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);
    let good = alice.tx(&s, transfer(bob.addr, MIN_RESERVE), true);
    let bad = alice.tx(&s, transfer(bob.addr, 1_000 * UNITS_PER_COIN), false);
    let before = s.clone();
    let err = s.apply_block(&block(1, &[good.clone(), bad], &p, 1_000), &FakePow);
    assert!(matches!(err, Err(BlockError::InvalidTx { index: 1, .. })));
    assert_eq!(s, before);

    assert!(matches!(
        s.apply_block(&block(2, &[], &p, 0), &FakePow),
        Err(BlockError::BadHeight { .. })
    ));
    s.apply_block(&block(1, &[good], &p, encode(&()).len() as u64), &FakePow)
        .unwrap();
    assert_eq!(s.height, 1);
    assert_eq!(s.mint().pot(), MINT_PER_BLOCK);
    assert_eq!(s.pending_rewards(), 0);
    assert!(s.check_invariant());
}

#[test]
fn base_fee_follows_block_fullness() {
    let mut s = State::genesis(CHAIN, Params::default());
    let p = Address::from_bytes([9; 20]);
    let max = s.params.max_block_bytes;
    s.apply_block(&block(1, &[], &p, max), &FakePow).unwrap();
    let up = s.base_fee_per_byte();
    assert!(up > MIN_BASE_FEE_PER_BYTE);
    s.apply_block(&block(2, &[], &p, 0), &FakePow).unwrap();
    assert!(s.base_fee_per_byte() < up);
    for h in 3..100 {
        s.apply_block(&block(h, &[], &p, 0), &FakePow).unwrap();
    }
    assert_eq!(s.base_fee_per_byte(), MIN_BASE_FEE_PER_BYTE);
}

#[test]
fn issuance_switches_from_mint_to_forge_and_respects_cap() {
    assert_eq!(block_issuance(1, 0), MINT_PER_BLOCK);
    assert_eq!(block_issuance(MINT_PHASE_BLOCKS, 0), MINT_PER_BLOCK);
    let after = MINT_PER_BLOCK * MINT_PHASE_BLOCKS;
    assert_eq!(
        block_issuance(MINT_PHASE_BLOCKS + 1, after),
        forge_amount(after)
    );
    // Environ 9,5 pièces par bloc juste après la bascule.
    let f = forge_amount(after);
    assert!(
        f > 949 * UNITS_PER_COIN / 100 && f < 950 * UNITS_PER_COIN / 100,
        "{f}"
    );
    assert_eq!(block_issuance(1, MAX_SUPPLY - 5), 5);
    assert_eq!(block_issuance(MINT_PHASE_BLOCKS + 1, MAX_SUPPLY), 0);
}

#[test]
fn state_root_changes_with_state() {
    let mut alice = User::new();
    let p = Address::from_bytes([9; 20]);
    let mut s = funded(&alice, 10);
    let r0 = s.state_root();
    s.apply_tx(
        &alice.tx(
            &s,
            transfer(Address::from_bytes([1; 20]), MIN_RESERVE),
            true,
        ),
        &p,
    )
    .unwrap();
    assert_ne!(s.state_root(), r0);
    assert_eq!(s.clone().state_root(), s.state_root());
}
