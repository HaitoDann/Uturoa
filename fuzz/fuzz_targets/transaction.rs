//! Une transaction quelconque, appliquée à un état avec des comptes
//! approvisionnés, ne fait jamais planter l'état et ne viole jamais
//! l'invariant de l'offre (soldes + réserves = offre ≤ plafond).
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;
use uturoa_core::{Address, Params, SignedTransaction, UNITS_PER_COIN};
use uturoa_crypto::KeyPair;
use uturoa_state::State;

fn base() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(|| {
        let mut s = State::genesis("utu-fuzz", Params::default());
        // Comptes déterministes, pour que le fuzzeur retrouve leurs adresses.
        for i in 0..4u8 {
            let kp = KeyPair::from_secret_bytes(&[i + 1; 32]);
            s.credit_for_tests(&Address::from_public_key(&kp.public_key()), 1_000 * UNITS_PER_COIN);
            s.credit_for_tests(&Address::from_bytes([i; 20]), 1_000 * UNITS_PER_COIN);
        }
        s
    })
}

fuzz_target!(|bytes: &[u8]| {
    let Ok(tx) = borsh::from_slice::<SignedTransaction>(bytes) else {
        return;
    };
    let mut s = base().clone();
    let before = s.clone();
    let result = s.apply_tx(&tx, &Address::from_bytes([9; 20]));
    assert!(s.check_invariant(), "invariant violé");
    if result.is_err() {
        assert_eq!(s, before, "une transaction refusée a modifié l'état");
    }
});
