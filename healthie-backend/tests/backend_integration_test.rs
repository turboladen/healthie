//! Wire-level tests: drive the production `api::router` through `tower::oneshot`
//! — no network socket. Mirrors the healthie-mcp integration harness.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::Value;
use tower::ServiceExt;

/// Build a router over a fresh in-memory migrated DB.
async fn app() -> axum::Router {
    let db = healthie_shared::test_support::test_db().await;
    healthie_backend::api::router(healthie_backend::AppState { db: db.clone() }, db)
}

/// Build a router over a fresh DB with an `Ingest` token provisioned, returning
/// the plaintext token and a DB handle for post-request assertions.
async fn app_with_ingest_token() -> (axum::Router, String, sea_orm::DatabaseConnection) {
    let db = healthie_shared::test_support::test_db().await;
    let token = healthie_shared::services::auth_token::provision(
        &db,
        healthie_shared::entities::auth_token::TokenKind::Ingest,
    )
    .await
    .expect("provision ingest token")
    .plaintext;
    let router =
        healthie_backend::api::router(healthie_backend::AppState { db: db.clone() }, db.clone());
    (router, token, db)
}

fn ingest_req(token: Option<&str>, body: &Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/ingest/hae")
        .header("content-type", "application/json");
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn health_reports_ok_and_version() {
    let resp = app()
        .await
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["version"].is_string(), "version must be a string");
}

#[tokio::test]
async fn ingest_requires_bearer_and_returns_204() {
    use sea_orm::EntityTrait;

    let (app, token, db) = app_with_ingest_token().await;
    let payload = serde_json::json!({ "data": { "metrics": [
        { "name": "weight_body_mass", "units": "lb",
          "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 234.0 }] }
    ] } });

    // No token → 401.
    let resp = app
        .clone()
        .oneshot(ingest_req(None, &payload))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // With the ingest token → 204, and the row lands (proves the full
    // HTTP → extractor → service → txn-commit path persists).
    let resp = app
        .clone()
        .oneshot(ingest_req(Some(&token), &payload))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let rows = healthie_shared::entities::daily_metric::Entity::find()
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "the weight point must persist");
    assert!((rows[0].value - 234.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn ingest_rejects_mcp_kind_token() {
    // An Mcp-kind token must NOT authorize ingest — proven at the HTTP boundary
    // WHILE a valid ingest token also exists, so the 401 is a cross-kind
    // rejection, not merely "no ingest token provisioned" (ADR-0005
    // blast-radius separation).
    let (app, ingest_token, db) = app_with_ingest_token().await;
    let mcp = healthie_shared::services::auth_token::provision(
        &db,
        healthie_shared::entities::auth_token::TokenKind::Mcp,
    )
    .await
    .expect("provision mcp token")
    .plaintext;
    assert_ne!(ingest_token, mcp, "the two kinds get distinct plaintexts");

    let payload = serde_json::json!({ "data": { "metrics": [] } });

    // The Mcp token is rejected even though a valid Ingest token is provisioned.
    let resp = app
        .clone()
        .oneshot(ingest_req(Some(&mcp), &payload))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "an mcp token must not drive ingest even when an ingest token exists"
    );

    // Positive control on the same app: the real ingest token still authorizes.
    let resp = app
        .oneshot(ingest_req(Some(&ingest_token), &payload))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "the ingest token must still authorize ingest"
    );
}

#[tokio::test]
async fn health_needs_no_token() {
    let (app, _token, _db) = app_with_ingest_token().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "health must not sit behind ingest auth"
    );
}

#[tokio::test]
async fn mcp_endpoint_is_mounted_and_guarded() {
    // POST /mcp without its own bearer → 401 from the MCP router's auth layer.
    let (app, _token, _db) = app_with_ingest_token().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
