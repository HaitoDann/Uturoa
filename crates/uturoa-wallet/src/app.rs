//! Serveur local du portefeuille et ses routes.
//!
//! - Les routes `/api/*` exigent l'en-tête `x-app-token`, un jeton aléatoire
//!   inséré dans la page : un autre site ouvert dans le navigateur ne peut pas
//!   les appeler.
//! - Après déverrouillage, la graine reste en mémoire jusqu'au verrouillage,
//!   automatique après 15 minutes sans activité.
//! - Avant chaque envoi, la page demande un aperçu (frais, total, solde après,
//!   mises en garde) ; les mêmes vérifications sont refaites à l'envoi.

use crate::amount::{format_coins, parse_coins};
use crate::miner::Miner;
use crate::node::Node;
use crate::vault::{self, Vault};
use anyhow::{Result, anyhow, bail};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use uturoa_core::{
    Address, Amount, MAX_MEMO_BYTES, MIN_RESERVE, PublicKey, SignedTransaction, Transaction,
    TxBody, encode,
};
use uturoa_crypto::KeyPair;
use uturoa_keystore::encoding::key_from_hex;
use uturoa_keystore::keyfile::{DEFAULT_KDF, Kdf};

const PAGE: &str = include_str!("app.html");
/// Verrouillage automatique après cette durée sans activité.
pub const LOCK_AFTER: Duration = Duration::from_secs(15 * 60);

pub struct AppState {
    dir: PathBuf,
    token: String,
    kdf: Kdf,
    vault: Option<Vault>,
    seed: Option<[u8; 64]>,
    last_activity: Instant,
    miner: Miner,
}

pub type Shared = Arc<Mutex<AppState>>;

fn lock(s: &Shared) -> MutexGuard<'_, AppState> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// Jeton aléatoire de 128 bits, en hexadécimal.
pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Erreur d'une route : code HTTP et message, transformés en JSON. Un type
/// compact plutôt qu'une `Response` entière.
pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

fn fail(code: StatusCode, message: impl std::fmt::Display) -> ApiError {
    ApiError(code, message.to_string())
}

fn bad(e: impl std::fmt::Display) -> ApiError {
    fail(StatusCode::BAD_REQUEST, e)
}

impl AppState {
    fn node(&self) -> Node {
        Node::new(
            self.vault
                .as_ref()
                .map_or(vault::DEFAULT_NODE.to_owned(), |v| v.node.clone()),
        )
    }

    fn key(&self, index: usize) -> Result<KeyPair> {
        let seed = self
            .seed
            .ok_or_else(|| anyhow!("portefeuille verrouillé"))?;
        let count = self.vault.as_ref().map_or(0, |v| v.accounts.len());
        if index >= count {
            bail!("compte inconnu");
        }
        Ok(vault::derive_account(
            &seed,
            u32::try_from(index).map_err(|_| anyhow!("compte inconnu"))?,
        ))
    }

    fn accounts(&self) -> Vec<Value> {
        let Some(v) = &self.vault else {
            return Vec::new();
        };
        v.accounts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let address = self
                    .key(i)
                    .ok()
                    .map(|k| Address::from_public_key(&k.public_key()).to_string());
                json!({ "index": i, "label": a.label, "address": address })
            })
            .collect()
    }
}

pub fn router(dir: PathBuf, token: String) -> Router {
    router_with_kdf(dir, token, DEFAULT_KDF)
}

pub fn router_with_kdf(dir: PathBuf, token: String, kdf: Kdf) -> Router {
    let vault = Vault::load(&dir).ok().flatten();
    let shared: Shared = Arc::new(Mutex::new(AppState {
        dir,
        token,
        kdf,
        vault,
        seed: None,
        last_activity: Instant::now(),
        miner: Miner::default(),
    }));
    let api = Router::new()
        .route("/state", get(state))
        .route("/phrase", get(phrase))
        .route("/create", post(create))
        .route("/unlock", post(unlock))
        .route("/lock", post(lock_route))
        .route("/reveal", post(reveal))
        .route("/accounts", post(add_account))
        .route("/node", post(set_node))
        .route("/overview", get(overview))
        .route("/validators", get(validators))
        .route("/preview", post(preview))
        .route("/send", post(send))
        .route("/delegate", post(delegate))
        .route("/miner", get(miner_status))
        .route("/miner/start", post(miner_start))
        .route("/miner/stop", post(miner_stop))
        .route_layer(axum::middleware::from_fn_with_state(shared.clone(), guard));
    Router::new()
        .route("/", get(page))
        .nest("/api", api)
        .with_state(shared)
}

async fn page(State(s): State<Shared>) -> Html<String> {
    Html(PAGE.replace("__APP_TOKEN__", &lock(&s).token))
}

/// Jeton obligatoire et verrouillage automatique.
async fn guard(State(s): State<Shared>, headers: HeaderMap, req: Request, next: Next) -> Response {
    {
        let mut st = lock(&s);
        let token = headers
            .get("x-app-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if token != st.token {
            return fail(StatusCode::FORBIDDEN, "jeton absent ou invalide").into_response();
        }
        if st.seed.is_some() && st.last_activity.elapsed() > LOCK_AFTER {
            st.seed = None;
        }
        st.last_activity = Instant::now();
    }
    next.run(req).await
}

async fn state(State(s): State<Shared>) -> Json<Value> {
    let st = lock(&s);
    Json(json!({
        "created": st.vault.is_some(),
        "unlocked": st.seed.is_some(),
        "node": st.node().addr,
        "accounts": st.accounts(),
    }))
}

async fn phrase() -> ApiResult {
    vault::new_phrase()
        .map(|p| Json(json!({ "phrase": p })))
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[derive(Deserialize)]
struct CreateReq {
    phrase: String,
    password: String,
}

async fn create(State(s): State<Shared>, Json(r): Json<CreateReq>) -> ApiResult {
    let mut st = lock(&s);
    if st.vault.is_some() {
        return Err(bad("un portefeuille existe déjà dans ce dossier"));
    }
    let v = Vault::create(&r.phrase, &r.password, st.kdf).map_err(bad)?;
    v.save(&st.dir).map_err(bad)?;
    st.seed = Some(v.unlock(&r.password).map_err(bad)?);
    st.vault = Some(v);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct PasswordReq {
    password: String,
}

async fn unlock(State(s): State<Shared>, Json(r): Json<PasswordReq>) -> ApiResult {
    let mut st = lock(&s);
    let seed = st
        .vault
        .as_ref()
        .ok_or_else(|| bad("aucun portefeuille"))?
        .unlock(&r.password)
        .map_err(|e| fail(StatusCode::UNAUTHORIZED, e))?;
    st.seed = Some(seed);
    Ok(Json(json!({ "ok": true })))
}

async fn lock_route(State(s): State<Shared>) -> Json<Value> {
    lock(&s).seed = None;
    Json(json!({ "ok": true }))
}

async fn reveal(State(s): State<Shared>, Json(r): Json<PasswordReq>) -> ApiResult {
    let st = lock(&s);
    let phrase = st
        .vault
        .as_ref()
        .ok_or_else(|| bad("aucun portefeuille"))?
        .reveal(&r.password)
        .map_err(|e| fail(StatusCode::UNAUTHORIZED, e))?;
    Ok(Json(json!({ "phrase": phrase })))
}

#[derive(Deserialize)]
struct LabelReq {
    label: String,
}

async fn add_account(State(s): State<Shared>, Json(r): Json<LabelReq>) -> ApiResult {
    let mut st = lock(&s);
    if st.seed.is_none() {
        return Err(bad("portefeuille verrouillé"));
    }
    let label = r.label.trim().chars().take(40).collect::<String>();
    let dir = st.dir.clone();
    let v = st.vault.as_mut().ok_or_else(|| bad("aucun portefeuille"))?;
    v.accounts.push(vault::AccountEntry {
        label: if label.is_empty() {
            format!("Compte {}", v.accounts.len().saturating_add(1))
        } else {
            label
        },
    });
    v.save(&dir).map_err(bad)?;
    Ok(Json(json!({ "accounts": st.accounts() })))
}

#[derive(Deserialize)]
struct NodeReq {
    node: String,
}

async fn set_node(State(s): State<Shared>, Json(r): Json<NodeReq>) -> ApiResult {
    let mut st = lock(&s);
    let dir = st.dir.clone();
    let v = st.vault.as_mut().ok_or_else(|| bad("aucun portefeuille"))?;
    let node = r
        .node
        .trim()
        .trim_start_matches("http://")
        .trim_end_matches('/');
    if node.is_empty() {
        return Err(bad("adresse du nœud vide"));
    }
    v.node = node.to_owned();
    v.save(&dir).map_err(bad)?;
    Ok(Json(json!({ "node": node })))
}

fn amount_of(v: &Value, field: &str) -> Amount {
    v.get(field)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

async fn overview(State(s): State<Shared>) -> ApiResult {
    let (node, accounts) = {
        let st = lock(&s);
        if st.seed.is_none() {
            return Err(bad("portefeuille verrouillé"));
        }
        (st.node(), st.accounts())
    };
    let result = tokio::task::spawn_blocking(move || {
        let status = node.status();
        let mut total: Amount = 0;
        let mut list = Vec::new();
        for a in accounts {
            let address = a.get("address").and_then(Value::as_str).unwrap_or_default();
            let info = node.account(address).ok();
            let balance = info.as_ref().map_or(0, |i| amount_of(i, "balance"));
            total = total.saturating_add(balance);
            list.push(json!({
                "index": a.get("index").cloned(),
                "label": a.get("label").cloned(),
                "address": address,
                "balance": balance.to_string(),
                "balance_text": format_coins(balance),
            }));
        }
        json!({
            "node_ok": status.is_ok(),
            "node_error": status.as_ref().err().map(|e| format!("{e:#}")),
            "status": status.ok(),
            "accounts": list,
            "total": total.to_string(),
            "total_text": format_coins(total),
        })
    })
    .await
    .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

async fn validators(State(s): State<Shared>) -> ApiResult {
    let node = lock(&s).node();
    tokio::task::spawn_blocking(move || node.validators())
        .await
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map(Json)
        .map_err(|e| fail(StatusCode::BAD_GATEWAY, e))
}

#[derive(Deserialize, Clone)]
struct SendReq {
    from: usize,
    to: String,
    amount: String,
    #[serde(default)]
    memo: String,
}

/// Transaction signée prête à envoyer, avec son aperçu.
struct Prepared {
    tx: SignedTransaction,
    preview: Value,
}

/// Construit et vérifie une transaction de `key` ; `debit` est le montant
/// retiré du solde en plus des frais.
fn prepare(
    node: &Node,
    key: &KeyPair,
    body: TxBody,
    debit: Amount,
    mut warnings: Vec<String>,
) -> Result<Prepared> {
    let sender = Address::from_public_key(&key.public_key());
    let status = node.status()?;
    let account = node.account(&sender.to_string())?;
    let chain_id = status
        .get("chain_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let base_fee = amount_of(&status, "base_fee_per_byte");
    let balance = amount_of(&account, "balance");
    let nonce = account
        .get("nonce")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1);
    let include_key = !account
        .get("key_known")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut tx = Transaction {
        chain_id,
        sender,
        nonce,
        fee: 0,
        body,
    };
    let len = Amount::try_from(encode(&tx.clone().sign(key, include_key)).len()).unwrap_or(0);
    // Marge de 2 : le frais de base peut monter avant l'inclusion.
    tx.fee = base_fee.saturating_mul(len).saturating_mul(2);
    let total = debit
        .checked_add(tx.fee)
        .ok_or_else(|| anyhow!("montant trop grand"))?;
    let remaining = balance.checked_sub(total).ok_or_else(|| {
        anyhow!(
            "solde insuffisant : {} pièces disponibles",
            format_coins(balance)
        )
    })?;
    if remaining != 0 && remaining < MIN_RESERVE {
        bail!(
            "il resterait {} pièce : un solde doit être nul ou d'au moins {} pièce",
            format_coins(remaining),
            format_coins(MIN_RESERVE)
        );
    }
    if remaining == 0 {
        warnings.push("Ce compte sera vidé entièrement.".into());
    }
    let preview = json!({
        "fee": tx.fee.to_string(),
        "fee_text": format_coins(tx.fee),
        "total_text": format_coins(total),
        "remaining_text": format_coins(remaining),
        "warnings": warnings,
    });
    Ok(Prepared {
        tx: tx.sign(key, include_key),
        preview,
    })
}

fn prepare_send(node: &Node, key: &KeyPair, r: &SendReq) -> Result<Prepared> {
    let to: Address =
        r.to.trim()
            .parse()
            .map_err(|e| anyhow!("adresse invalide : {e}"))?;
    let amount = parse_coins(&r.amount)?;
    if amount == 0 {
        bail!("le montant doit être positif");
    }
    if to == Address::from_public_key(&key.public_key()) {
        bail!("impossible d'envoyer à soi-même");
    }
    if r.memo.len() > MAX_MEMO_BYTES {
        bail!("mémo trop long ({MAX_MEMO_BYTES} octets au plus)");
    }
    let mut warnings = Vec::new();
    let dest = node.account(&to.to_string())?;
    let fresh =
        amount_of(&dest, "balance") == 0 && dest.get("nonce").and_then(Value::as_u64) == Some(0);
    if fresh {
        if amount < MIN_RESERVE {
            bail!(
                "ce destinataire n'a encore jamais rien reçu : il faut lui envoyer au moins {} pièce",
                format_coins(MIN_RESERVE)
            );
        }
        warnings.push("Ce destinataire n'a encore jamais rien reçu : vérifiez l'adresse.".into());
    }
    let memo = (!r.memo.is_empty()).then(|| r.memo.clone().into_bytes());
    prepare(
        node,
        key,
        TxBody::Transfer { to, amount, memo },
        amount,
        warnings,
    )
}

async fn run_prepared(
    s: &Shared,
    from: usize,
    submit: bool,
    build: impl FnOnce(&Node, &KeyPair) -> Result<Prepared> + Send + 'static,
) -> ApiResult {
    let (node, key) = {
        let st = lock(s);
        (st.node(), st.key(from).map_err(bad)?)
    };
    tokio::task::spawn_blocking(move || -> Result<Value> {
        let p = build(&node, &key)?;
        if submit {
            let id = node.submit_tx(&p.tx)?;
            Ok(json!({ "id": id, "preview": p.preview }))
        } else {
            Ok(p.preview)
        }
    })
    .await
    .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map(Json)
    .map_err(bad)
}

async fn preview(State(s): State<Shared>, Json(r): Json<SendReq>) -> ApiResult {
    let from = r.from;
    run_prepared(&s, from, false, move |n, k| prepare_send(n, k, &r)).await
}

async fn send(State(s): State<Shared>, Json(r): Json<SendReq>) -> ApiResult {
    let from = r.from;
    run_prepared(&s, from, true, move |n, k| prepare_send(n, k, &r)).await
}

#[derive(Deserialize)]
struct DelegateReq {
    from: usize,
    validator: String,
    amount: String,
    #[serde(default)]
    submit: bool,
}

async fn delegate(State(s): State<Shared>, Json(r): Json<DelegateReq>) -> ApiResult {
    let from = r.from;
    let submit = r.submit;
    run_prepared(&s, from, submit, move |n, k| {
        let validator: PublicKey = key_from_hex(&r.validator)?;
        let amount = parse_coins(&r.amount)?;
        if amount == 0 {
            bail!("le montant doit être positif");
        }
        prepare(
            n,
            k,
            TxBody::Delegate { validator, amount },
            amount,
            vec!["Les pièces déléguées sont rendues 21 jours après leur retrait.".into()],
        )
    })
    .await
}

async fn miner_status(State(s): State<Shared>) -> Json<Value> {
    Json(json!(lock(&s).miner.status()))
}

#[derive(Deserialize)]
struct MinerReq {
    account: usize,
    threads: usize,
}

async fn miner_start(State(s): State<Shared>, Json(r): Json<MinerReq>) -> ApiResult {
    let mut st = lock(&s);
    let key = st.key(r.account).map_err(bad)?;
    let node = st.node();
    let beneficiary = Address::from_public_key(&key.public_key());
    st.miner.start(node, beneficiary, r.threads);
    Ok(Json(json!(st.miner.status())))
}

async fn miner_stop(State(s): State<Shared>) -> Json<Value> {
    let mut st = lock(&s);
    st.miner.stop();
    Json(json!(st.miner.status()))
}

/// Nombre de cœurs proposé par défaut au mineur.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}
