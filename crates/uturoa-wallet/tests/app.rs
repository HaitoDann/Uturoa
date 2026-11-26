#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

//! Routes du portefeuille, sans nœud.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uturoa_keystore::keyfile::Kdf;
use uturoa_wallet::app::router_with_kdf;

const FAST: Kdf = Kdf {
    m_cost_kib: 8,
    t_cost: 1,
    p_cost: 1,
};

async fn call(
    app: &axum::Router,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-app-token", token)
        .header("content-type", "application/json")
        .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let code = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn token_is_required_and_page_embeds_it() {
    let dir = tempfile::tempdir().unwrap();
    let app = router_with_kdf(dir.path().into(), "jeton".into(), FAST);
    let (code, _) = call(&app, "faux", "GET", "/api/state", None).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, s) = call(&app, "jeton", "GET", "/api/state", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(s["created"], false);
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let html =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(html.contains("\"jeton\""));
}

#[tokio::test]
async fn create_lock_unlock_and_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let app = router_with_kdf(dir.path().into(), "t".into(), FAST);
    let (_, p) = call(&app, "t", "GET", "/api/phrase", None).await;
    let phrase = p["phrase"].as_str().unwrap().to_owned();
    // Phrase invalide, puis mot de passe trop court.
    let (code, _) = call(
        &app,
        "t",
        "POST",
        "/api/create",
        Some(json!({"phrase": "un deux trois", "password": "motdepasse"})),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = call(
        &app,
        "t",
        "POST",
        "/api/create",
        Some(json!({"phrase": phrase, "password": "court"})),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = call(
        &app,
        "t",
        "POST",
        "/api/create",
        Some(json!({"phrase": phrase, "password": "motdepasse"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let (_, s) = call(&app, "t", "GET", "/api/state", None).await;
    assert_eq!(s["unlocked"], true);
    let first = s["accounts"][0]["address"].as_str().unwrap().to_owned();
    assert!(first.starts_with("utu1"));
    // Une seconde création est refusée.
    let (code, _) = call(
        &app,
        "t",
        "POST",
        "/api/create",
        Some(json!({"phrase": phrase, "password": "motdepasse"})),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    let (_, a) = call(
        &app,
        "t",
        "POST",
        "/api/accounts",
        Some(json!({"label": "Épargne"})),
    )
    .await;
    assert_eq!(a["accounts"].as_array().unwrap().len(), 2);

    call(&app, "t", "POST", "/api/lock", Some(json!({}))).await;
    let (_, s) = call(&app, "t", "GET", "/api/state", None).await;
    assert_eq!(s["unlocked"], false);
    assert!(s["accounts"][0]["address"].is_null());
    let (code, _) = call(
        &app,
        "t",
        "POST",
        "/api/unlock",
        Some(json!({"password": "mauvais"})),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    let (code, _) = call(
        &app,
        "t",
        "POST",
        "/api/unlock",
        Some(json!({"password": "motdepasse"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let (_, r) = call(
        &app,
        "t",
        "POST",
        "/api/reveal",
        Some(json!({"password": "motdepasse"})),
    )
    .await;
    assert_eq!(r["phrase"], phrase);

    // Un autre portefeuille restauré avec la même phrase retrouve les mêmes comptes.
    let dir2 = tempfile::tempdir().unwrap();
    let app2 = router_with_kdf(dir2.path().into(), "u".into(), FAST);
    call(
        &app2,
        "u",
        "POST",
        "/api/create",
        Some(json!({"phrase": phrase.to_uppercase(), "password": "autremotdepasse"})),
    )
    .await;
    let (_, s2) = call(&app2, "u", "GET", "/api/state", None).await;
    assert_eq!(s2["accounts"][0]["address"], first);

    // Sans nœud joignable : erreur claire, pas de plantage.
    call(
        &app,
        "t",
        "POST",
        "/api/node",
        Some(json!({"node": "127.0.0.1:1"})),
    )
    .await;
    let (code, o) = call(&app, "t", "GET", "/api/overview", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(o["node_ok"], false);
    let (code, e) = call(
        &app,
        "t",
        "POST",
        "/api/preview",
        Some(json!({"from": 0, "to": first, "amount": "1"})),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        e["error"].as_str().unwrap().contains("soi-même")
            || e["error"].as_str().unwrap().contains("injoignable")
    );
}
