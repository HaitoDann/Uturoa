//! Tout message venu d'un pair (consensus, transactions, parts de frappe,
//! rattrapage, instantané d'état) est décodé sans planter, et seul
//! l'encodage canonique est accepté.
#![no_main]

use borsh::{BorshDeserialize, BorshSerialize};
use libfuzzer_sys::fuzz_target;
use uturoa_consensus::Evidence;
use uturoa_core::{Block, CommitCertificate, MintShare, SignedTransaction};
use uturoa_node::network::{NetMessage, SyncRequest, SyncResponse};
use uturoa_state::State;

fn canonical<T: BorshDeserialize + BorshSerialize>(bytes: &[u8]) {
    if let Ok(v) = borsh::from_slice::<T>(bytes) {
        assert_eq!(borsh::to_vec(&v).unwrap(), bytes, "encodage non canonique accepté");
    }
}

fuzz_target!(|bytes: &[u8]| {
    canonical::<NetMessage>(bytes);
    canonical::<SyncRequest>(bytes);
    canonical::<SyncResponse>(bytes);
    canonical::<Block>(bytes);
    canonical::<CommitCertificate>(bytes);
    canonical::<SignedTransaction>(bytes);
    canonical::<MintShare>(bytes);
    canonical::<Evidence>(bytes);
    // Un état (instantané) n'est accepté que par décodage canonique.
    if let Ok(state) = uturoa_core::decode_canonical::<State>(bytes) {
        assert_eq!(borsh::to_vec(&state).unwrap(), bytes);
    }
});
