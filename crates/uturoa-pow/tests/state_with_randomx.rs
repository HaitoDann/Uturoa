#![allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]

//! Bout à bout : une part minée avec le vrai RandomX est acceptée par l'état,
//! puis payée à la fin de la période.

use uturoa_core::{Address, MINT_PER_BLOCK, MintShare, Params};
use uturoa_crypto::{Domain, hash_tagged_parts};
use uturoa_pow::{RandomXHasher, mine};
use uturoa_state::mint::MINT_PERIOD;
use uturoa_state::{BlockInput, State};

#[test]
fn real_randomx_share_is_accepted_and_paid() {
    let pow = RandomXHasher::new();
    let proposer = Address::from_bytes([9; 20]);
    let miner = Address::from_bytes([1; 20]);
    let parent = |h: u64| hash_tagged_parts(Domain::Block, &[&h.to_le_bytes()]);
    let mut s = State::genesis_with_mint_difficulty("utu-test", Params::default(), 4);
    let apply = |s: &mut State, h: u64, shares: &[MintShare]| {
        s.apply_block(
            &BlockInput {
                height: h,
                parent_hash: parent(h - 1),
                txs: &[],
                shares,
                proposer: &proposer,
                last_commit: None,
                block_bytes: 0,
            },
            &pow,
        )
    };
    apply(&mut s, 1, &[]).unwrap();
    let template = MintShare {
        chain_id: "utu-test".into(),
        beneficiary: miner,
        anchor_height: 1,
        anchor_hash: parent(1),
        nonce: 0,
    };
    let seed = s.mint().seed_for(2).unwrap();
    let share = mine(&RandomXHasher::new(), &seed, &template, 4, 1_000).unwrap();
    apply(&mut s, 2, &[share]).unwrap();
    for h in 3..=MINT_PERIOD {
        apply(&mut s, h, &[]).unwrap();
    }
    assert_eq!(s.balance(&miner), MINT_PER_BLOCK * MINT_PERIOD);
}
