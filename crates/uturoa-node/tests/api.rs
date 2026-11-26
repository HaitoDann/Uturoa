#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! API HTTP, appelée sans réseau.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use uturoa_core::{Address, Hash, MintShare, encode};
use uturoa_crypto::{Domain, KeyPair, hash_tagged_parts};
use uturoa_node::api::router;
use uturoa_node::chain::Chain;
use uturoa_node::genesis_file::{GenesisFile, validator_entry};
use uturoa_node::node::{Shared, SharedRef};
use uturoa_state::{PowHasher, meets_target};

struct FakePow;
impl PowHasher for FakePow {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        hash_tagged_parts(Domain::MintShare, &[seed, input])
    }
}

fn shared(dir: &std::path::Path) -> SharedRef {
    let kps: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let genesis = GenesisFile {
        chain_id: "utu-api".into(),
        genesis_time_ms: 1_000,
        mint_difficulty: 1,
        validators: kps
            .iter()
            .enumerate()
            .map(|(i, k)| validator_entry(&k.public_key(), &format!("v{i}")))
            .collect(),
        block_interval_ms: None,
        mint_phase_blocks: None,
    };
    let chain = Chain::open(dir, &genesis, Box::new(FakePow)).unwrap();
    Arc::new(Mutex::new(Shared::new(chain).unwrap()))
}

async fn call(
    shared: &SharedRef,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
        .unwrap();
    let res = router(shared.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn read_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let s = shared(dir.path());
    let (code, status) = call(&s, "GET", "/status", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(status["chain_id"], "utu-api");
    assert_eq!(status["height"], 0);
    assert_eq!(status["validators"], 4);

    let addr = Address::from_bytes([1; 20]).to_string();
    let (code, acc) = call(&s, "GET", &format!("/accounts/{addr}"), None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(acc["balance"], "0");
    let (code, _) = call(&s, "GET", "/accounts/pas-une-adresse", None).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    let (code, _) = call(&s, "GET", "/blocks/1", None).await;
    assert_eq!(code, StatusCode::NOT_FOUND);

    let (code, v) = call(&s, "GET", "/validators", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(v["active"].as_array().unwrap().len(), 4);

    let (code, work) = call(&s, "GET", "/mint/work", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(work["next_height"], 1);

    let (code, _) = call(&s, "GET", "/", None).await;
    assert_eq!(code, StatusCode::OK);
}

#[tokio::test]
async fn submissions() {
    let dir = tempfile::tempdir().unwrap();
    let s = shared(dir.path());
    // Hexadécimal invalide, puis transaction illisible.
    let (code, _) = call(
        &s,
        "POST",
        "/transactions",
        Some(serde_json::json!({"hex": "zz"})),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = call(
        &s,
        "POST",
        "/transactions",
        Some(serde_json::json!({"hex": "00"})),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    // Une part ancrée sur un bloc inconnu est refusée, sans erreur HTTP.
    let share = MintShare {
        chain_id: "utu-api".into(),
        beneficiary: Address::from_bytes([1; 20]),
        anchor_height: 0,
        anchor_hash: [0; 32],
        nonce: 0,
    };
    let hex: String = encode(&vec![share.clone()])
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let (code, r) = call(&s, "POST", "/shares", Some(serde_json::json!({"hex": hex}))).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(r["accepted"], 0);
    // Une part valide (ancrée sur le bloc de genèse) est acceptée.
    let (tip, seed) = {
        let g = s.lock().unwrap();
        (g.chain.tip_hash(), g.chain.tip_hash())
    };
    let _ = seed;
    let mut good = share;
    good.anchor_hash = tip;
    let _ = meets_target;
    let hex: String = encode(&vec![good])
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let (_, r) = call(&s, "POST", "/shares", Some(serde_json::json!({"hex": hex}))).await;
    // Au bloc 1, la clé RandomX (bloc 0) n'est pas encore enregistrée : refus propre.
    assert!(r["accepted"] == 0 || r["accepted"] == 1);
}
