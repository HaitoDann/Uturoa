#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use uturoa_core::{Address, Hash, MINT_PER_BLOCK, MINT_PHASE_BLOCKS, MintShare, Params};
use uturoa_crypto::{Domain, hash_tagged_parts};
use uturoa_state::mint::{MINT_ANCHOR_WINDOW, MINT_PERIOD};
use uturoa_state::{BlockError, BlockInput, PowHasher, ShareError, State, meets_target};

const CHAIN: &str = "utu-test";

struct FakePow;
impl PowHasher for FakePow {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        hash_tagged_parts(Domain::MintShare, &[seed, input])
    }
}

fn parent(height: u64) -> Hash {
    hash_tagged_parts(Domain::Block, &[&height.to_le_bytes()])
}

/// Applique un bloc `height` contenant `shares`.
fn apply(s: &mut State, height: u64, shares: &[MintShare]) -> Result<(), BlockError> {
    let proposer = Address::from_bytes([9; 20]);
    s.apply_block(
        &BlockInput {
            height,
            parent_hash: parent(height - 1),
            txs: &[],
            shares,
            proposer: &proposer,
            last_commit: None,
            block_bytes: 0,
        },
        &FakePow,
    )
}

/// Cherche un nonce valide pour une part ancrée sur le bloc `anchor`, à
/// inclure dans le bloc `height`.
fn mine(s: &State, height: u64, anchor: u64, who: Address, start: u64) -> MintShare {
    let seed = s.mint().seed_for(height).unwrap();
    let mut share = MintShare {
        chain_id: CHAIN.into(),
        beneficiary: who,
        anchor_height: anchor,
        anchor_hash: parent(anchor),
        nonce: start,
    };
    while !meets_target(
        &FakePow.hash(&seed, &share.pow_input()),
        s.mint().difficulty,
    ) {
        share.nonce += 1;
    }
    share
}

fn fresh(difficulty: u64) -> State {
    let mut s = State::genesis_with_mint_difficulty(CHAIN, Params::default(), difficulty);
    apply(&mut s, 1, &[]).unwrap();
    s
}

#[test]
fn shares_are_paid_pro_rata_at_period_end() {
    let mut s = fresh(4);
    let alice = Address::from_bytes([1; 20]);
    let bob = Address::from_bytes([2; 20]);
    // Alice fournit 3 parts, Bob 1, dans le bloc 2.
    let mut shares = Vec::new();
    let mut n = 0;
    for _ in 0..3 {
        let sh = mine(&s, 2, 1, alice, n);
        n = sh.nonce + 1;
        shares.push(sh);
    }
    shares.push(mine(&s, 2, 1, bob, 0));
    apply(&mut s, 2, &shares).unwrap();
    assert_eq!(s.mint().period_total(), 4);
    for h in 3..=MINT_PERIOD {
        apply(&mut s, h, &[]).unwrap();
    }
    let pot = MINT_PER_BLOCK * MINT_PERIOD;
    assert_eq!(s.balance(&alice), pot * 3 / 4);
    assert_eq!(s.balance(&bob), pot / 4);
    assert_eq!(s.mint().period_total(), 0);
    assert_eq!(s.mint().pot(), pot - pot * 3 / 4 - pot / 4);
    assert!(s.check_invariant());
}

#[test]
fn pot_carries_over_when_nobody_mines() {
    let mut s = fresh(1);
    for h in 2..=MINT_PERIOD {
        apply(&mut s, h, &[]).unwrap();
    }
    assert_eq!(s.mint().pot(), MINT_PER_BLOCK * MINT_PERIOD);
    // Personne n'a frappé : la difficulté ne descend pas sous 1.
    assert_eq!(s.mint().difficulty, 1);
    let who = Address::from_bytes([1; 20]);
    let h = MINT_PERIOD + 1;
    let sh = mine(&s, h, h - 1, who, 0);
    apply(&mut s, h, &[sh]).unwrap();
    for h in MINT_PERIOD + 2..=2 * MINT_PERIOD {
        apply(&mut s, h, &[]).unwrap();
    }
    assert_eq!(s.balance(&who), MINT_PER_BLOCK * 2 * MINT_PERIOD);
}

#[test]
fn duplicate_share_is_rejected_even_in_a_later_block() {
    let mut s = fresh(1);
    let sh = mine(&s, 2, 1, Address::from_bytes([1; 20]), 0);
    let dup = apply(&mut s.clone(), 2, &[sh.clone(), sh.clone()]);
    assert!(matches!(
        dup,
        Err(BlockError::InvalidShare {
            index: 1,
            error: ShareError::Duplicate
        })
    ));
    apply(&mut s, 2, std::slice::from_ref(&sh)).unwrap();
    assert!(matches!(
        apply(&mut s, 3, &[sh]),
        Err(BlockError::InvalidShare {
            error: ShareError::Duplicate,
            ..
        })
    ));
}

#[test]
fn anchor_must_be_recent_and_match() {
    let mut s = fresh(1);
    for h in 2..=MINT_ANCHOR_WINDOW + 5 {
        apply(&mut s, h, &[]).unwrap();
    }
    let h = MINT_ANCHOR_WINDOW + 6;
    let who = Address::from_bytes([1; 20]);
    // Trop ancien.
    let old = mine(&s, h, 2, who, 0);
    assert!(matches!(
        apply(&mut s.clone(), h, &[old]),
        Err(BlockError::InvalidShare {
            error: ShareError::BadAnchor,
            ..
        })
    ));
    // Empreinte qui ne correspond pas.
    let mut wrong = mine(&s, h, h - 1, who, 0);
    wrong.anchor_hash = [0; 32];
    assert!(apply(&mut s.clone(), h, &[wrong]).is_err());
    // Ancrage futur.
    let mut future = mine(&s, h, h - 1, who, 0);
    future.anchor_height = h;
    assert!(apply(&mut s.clone(), h, &[future]).is_err());
    // Récent et correct.
    let sh = mine(&s, h, h - 1, who, 0);
    apply(&mut s, h, &[sh]).unwrap();
}

#[test]
fn wrong_chain_and_insufficient_work() {
    let s = fresh(1);
    let mut other = mine(&s, 2, 1, Address::from_bytes([1; 20]), 0);
    other.chain_id = "autre".into();
    assert!(matches!(
        apply(&mut s.clone(), 2, &[other]),
        Err(BlockError::InvalidShare {
            error: ShareError::WrongChain,
            ..
        })
    ));

    let mut hard = fresh(1 << 40);
    let lazy = MintShare {
        chain_id: CHAIN.into(),
        beneficiary: Address::from_bytes([1; 20]),
        anchor_height: 1,
        anchor_hash: parent(1),
        nonce: 0,
    };
    assert!(matches!(
        apply(&mut hard, 2, &[lazy]),
        Err(BlockError::InvalidShare {
            error: ShareError::InsufficientWork,
            ..
        })
    ));
}

#[test]
fn stealing_a_share_by_changing_the_beneficiary_fails() {
    let mut s = fresh(1 << 8);
    let mut stolen = mine(&s, 2, 1, Address::from_bytes([1; 20]), 0);
    stolen.beneficiary = Address::from_bytes([66; 20]);
    // Avec une difficulté de 256, la part modifiée n'a presque aucune chance
    // de rester valide.
    let seed = s.mint().seed_for(2).unwrap();
    let still_ok = meets_target(&FakePow.hash(&seed, &stolen.pow_input()), 1 << 8);
    assert_eq!(apply(&mut s, 2, &[stolen]).is_ok(), still_ok);
}

#[test]
fn too_many_shares_in_a_block() {
    let mut s = fresh(1);
    let max = s.params.max_block_shares as usize;
    let who = Address::from_bytes([1; 20]);
    let mut shares = Vec::new();
    let mut n = 0;
    for _ in 0..=max {
        let sh = mine(&s, 2, 1, who, n);
        n = sh.nonce + 1;
        shares.push(sh);
    }
    assert!(matches!(
        apply(&mut s, 2, &shares),
        Err(BlockError::InvalidShare {
            error: ShareError::TooMany,
            ..
        })
    ));
}

#[test]
fn difficulty_adjusts_each_period() {
    let mut s = fresh(10);
    let who = Address::from_bytes([1; 20]);
    let mut n = 0;
    for h in 2..=MINT_PERIOD {
        let sh = mine(&s, h, h - 1, who, n);
        n = sh.nonce + 1;
        apply(&mut s, h, &[sh]).unwrap();
    }
    // 49 parts pour une cible de 2 000 : la difficulté chute, au plus de 4 fois.
    assert_eq!(s.mint().difficulty, 10 / 4);
}

/// Parcourt toute la phase de frappe (1 314 000 blocs) : lent en debug.
#[test]
#[ignore = "long : cargo test -- --ignored"]
fn unshared_mint_returns_to_the_reservoir_and_shares_stop() {
    let mut s = fresh(1);
    for h in 2..=MINT_PHASE_BLOCKS {
        apply(&mut s, h, &[]).unwrap();
    }
    assert_eq!(s.mint().pot(), 0);
    assert_eq!(s.supply(), 0);
    let h = MINT_PHASE_BLOCKS + 1;
    let sh = mine(&s, h, h - 1, Address::from_bytes([1; 20]), 0);
    assert!(matches!(
        apply(&mut s, h, &[sh]),
        Err(BlockError::InvalidShare {
            error: ShareError::PhaseOver,
            ..
        })
    ));
    apply(&mut s, h, &[]).unwrap();
    assert!(s.pending_rewards() > 0);
}
