//! API HTTP (JSON) du nœud, et page d'exploration.
//!
//! Lecture : état du réseau, comptes, blocs, validateurs, travail de frappe.
//! Écriture : soumission de transactions et de parts de frappe (encodées en
//! borsh, en hexadécimal).
//!
//! Les montants sont des chaînes (en unités) : un entier de plus de 2⁵³ perd
//! sa précision en JavaScript.

use crate::network::NetMessage;
use crate::node::{SharedRef, lock};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use uturoa_core::{Address, MintShare, SignedTransaction, decode};
use uturoa_keystore::encoding::{hash_to_hex, key_to_hex};
use uturoa_state::ValidatorStatus;

const EXPLORER: &str = include_str!("explorer.html");

pub fn router(shared: SharedRef) -> Router {
    Router::new()
        .route("/", get(|| async { Html(EXPLORER) }))
        .route("/status", get(status))
        .route("/accounts/{address}", get(account))
        .route("/blocks/{height}", get(block))
        .route("/validators", get(validators))
        .route("/mint/work", get(mint_work))
        .route("/transactions", post(submit_tx))
        .route("/shares", post(submit_shares))
        .with_state(shared)
}

fn error(code: StatusCode, message: impl Into<String>) -> Response {
    (code, Json(json!({ "error": message.into() }))).into_response()
}

async fn status(State(shared): State<SharedRef>) -> Json<Value> {
    let s = lock(&shared);
    let st = s.chain.state();
    Json(json!({
        "chain_id": st.chain_id,
        "height": st.height,
        "tip": hash_to_hex(&s.chain.tip_hash()),
        "protocol_version": st.protocol_version,
        "supply": st.supply().to_string(),
        "pending_rewards": st.pending_rewards().to_string(),
        "base_fee_per_byte": st.base_fee_per_byte().to_string(),
        "validators": s.chain.next_validators().len(),
        "peers": s.peers,
        "mempool": s.mempool.len(),
        "pending_shares": s.mempool.shares_len(),
        "mint": {
            "difficulty": st.mint().difficulty,
            "pot": st.mint().pot().to_string(),
            "period_shares": st.mint().period_total(),
        },
        "blocks_since_start": s.blocks,
        "blocks_first_round": s.blocks_first_round,
    }))
}

async fn account(State(shared): State<SharedRef>, Path(address): Path<String>) -> Response {
    let Ok(addr) = address.parse::<Address>() else {
        return error(StatusCode::BAD_REQUEST, "adresse invalide");
    };
    let s = lock(&shared);
    let st = s.chain.state();
    let account = st.account(&addr);
    Json(json!({
        "address": addr.to_string(),
        "balance": account.map_or(0, |a| a.balance).to_string(),
        "nonce": account.map_or(0, |a| a.nonce),
        "key_known": account.is_some_and(|a| a.key.is_some()),
        "multisig": account.and_then(|a| a.multisig.as_ref()).map(|m| json!({
            "threshold": m.threshold,
            "signers": m.signers.iter().map(key_to_hex).collect::<Vec<_>>(),
        })),
    }))
    .into_response()
}

async fn block(State(shared): State<SharedRef>, Path(height): Path<String>) -> Response {
    let s = lock(&shared);
    let h = if height == "latest" {
        s.chain.height()
    } else {
        match height.parse::<u64>() {
            Ok(h) => h,
            Err(_) => return error(StatusCode::BAD_REQUEST, "hauteur invalide"),
        }
    };
    let block = match s.chain.store().block(h) {
        Ok(Some(b)) => b,
        Ok(None) => return error(StatusCode::NOT_FOUND, "bloc inconnu"),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let commit = s.chain.store().commit(h).ok().flatten();
    let hd = &block.header;
    Json(json!({
        "height": hd.height,
        "hash": hash_to_hex(&block.hash()),
        "prev_hash": hash_to_hex(&hd.prev_hash),
        "timestamp_ms": hd.timestamp,
        "proposer": hd.proposer,
        "state_root": hash_to_hex(&hd.state_root),
        "round": commit.as_ref().map(|c| c.round),
        "signatures": commit.as_ref().map(|c| c.signatures.len()),
        "transactions": block.txs.iter().map(|t| json!({
            "id": hash_to_hex(&t.id()),
            "sender": t.tx.sender.to_string(),
            "nonce": t.tx.nonce,
            "fee": t.tx.fee.to_string(),
        })).collect::<Vec<_>>(),
        "mint_shares": block.mint_shares.len(),
    }))
    .into_response()
}

async fn validators(State(shared): State<SharedRef>) -> Json<Value> {
    let s = lock(&shared);
    let st = s.chain.state();
    let active = s.chain.next_validators();
    let list: Vec<Value> = active
        .iter()
        .filter_map(|key| {
            let op = st.staking().operator_of(key)?;
            let v = st.staking().validator(&op)?;
            let status = match v.status {
                ValidatorStatus::Candidate => "actif",
                ValidatorStatus::Jailed { .. } => "mis à l'écart",
                ValidatorStatus::Left => "parti",
                ValidatorStatus::Banned => "exclu",
            };
            Some(json!({
                "operator": op.to_string(),
                "key": key_to_hex(key),
                "name": v.info.name,
                "site": v.info.site,
                "status": status,
                "bootstrap": v.bootstrap,
                "self_stake": v.self_stake.to_string(),
                "delegated": v.delegated.to_string(),
                "commission_bps": v.commission_bps,
                "max_commission_bps": v.max_commission_bps,
                "epoch_signed": v.epoch_signed,
                "epoch_expected": v.epoch_expected,
            }))
        })
        .collect();
    Json(json!({ "active": list }))
}

/// Ce dont un mineur a besoin pour calculer une part valide pour le prochain bloc.
async fn mint_work(State(shared): State<SharedRef>) -> Response {
    let s = lock(&shared);
    let st = s.chain.state();
    let next = st.height.saturating_add(1);
    let Some(seed) = crate::mempool::seed_for(st, next, &s.chain.tip_hash()) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "clé RandomX pas encore connue",
        );
    };
    Json(json!({
        "chain_id": st.chain_id,
        "next_height": next,
        "anchor_height": st.height,
        "anchor_hash": hash_to_hex(&s.chain.tip_hash()),
        "seed": hash_to_hex(&seed),
        "difficulty": st.mint().difficulty,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct Hex {
    hex: String,
}

async fn submit_tx(State(shared): State<SharedRef>, Json(body): Json<Hex>) -> Response {
    let Ok(bytes) = hex_decode(&body.hex) else {
        return error(StatusCode::BAD_REQUEST, "hexadécimal invalide");
    };
    let Ok(tx) = decode::<SignedTransaction>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "transaction illisible");
    };
    let id = tx.id();
    let mut guard = lock(&shared);
    let s = &mut *guard;
    match s.mempool.add(s.chain.state(), tx.clone()) {
        Ok(()) => {
            s.broadcast(NetMessage::Txs(vec![tx]));
            Json(json!({ "id": hash_to_hex(&id) })).into_response()
        }
        Err(e) => error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
    }
}

async fn submit_shares(State(shared): State<SharedRef>, Json(body): Json<Hex>) -> Response {
    let Ok(bytes) = hex_decode(&body.hex) else {
        return error(StatusCode::BAD_REQUEST, "hexadécimal invalide");
    };
    let Ok(shares) = decode::<Vec<MintShare>>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "parts illisibles");
    };
    let mut guard = lock(&shared);
    let s = &mut *guard;
    let tip = s.chain.tip_hash();
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for share in shares {
        match s
            .mempool
            .add_share(s.chain.state(), &tip, share.clone(), s.chain.pow())
        {
            Ok(()) => accepted.push(share),
            Err(e) => rejected.push(e.to_string()),
        }
    }
    let n = accepted.len();
    if n > 0 {
        s.broadcast(NetMessage::Shares(accepted));
    }
    Json(json!({ "accepted": n, "rejected": rejected })).into_response()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i.saturating_add(2))
                .and_then(|b| u8::from_str_radix(b, 16).ok())
                .ok_or(())
        })
        .collect()
}
