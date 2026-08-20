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

/// The deploy gate matches `"build":"<sha>"` in this body to decide whether the
/// binary it uploaded is the one answering. If the field ever stops being a
/// plain non-empty string carrying the embedded commit, every deploy fails —
/// or worse, passes while the old binary serves.
#[tokio::test]
async fn health_names_the_build_it_is_running() {
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

    let build = json["build"].as_str().expect("build must be a string");
    assert!(!build.is_empty(), "build must never be empty");
    assert_eq!(
        build,
        env!("GIT_COMMIT"),
        "the served build must be the one the build script embedded"
    );
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

/// The wire cannot carry a non-finite number, and the finite guard in
/// `services::metrics` rests on that.
///
/// `serde_json` rejects a float that overflows to infinity at parse
/// (`ErrorCode::NumberOutOfRange`) and `Number::from_f64` refuses to build a
/// non-finite `Value` at all, so a NaN or an infinity cannot reach `ingest_hae`
/// through HTTP — the request fails deserialization first. That makes the
/// guard defense in depth rather than a live fix, and this is what pins the
/// claim: if a future `serde_json`, or the `arbitrary_precision` feature, ever
/// makes it representable, this test fails and the guard's status changes from
/// belt-and-braces to load-bearing.
#[tokio::test]
async fn a_non_finite_value_cannot_cross_the_wire() {
    let (app, token, db) = app_with_ingest_token().await;
    let body = r#"{"data":{"metrics":[{"name":"weight_body_mass","units":"lb",
        "data":[{"date":"2026-07-28 00:00:00 -0700","qty":1e999}]}]}}"#;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ingest/hae")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    // 400 rather than 422: `NumberOutOfRange` is raised by the *tokenizer*, so
    // axum classifies it as malformed JSON rather than as valid JSON of the
    // wrong shape. Either way the handler never runs.
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an overflowing float must be refused by the extractor, not stored"
    );

    assert!(
        <healthie_shared::entities::daily_metric::Entity as sea_orm::EntityTrait>::find()
            .all(&db)
            .await
            .unwrap()
            .is_empty(),
        "nothing may reach daily_metric from a payload that never parsed"
    );
}

/// healthie-ei8 end to end: the conversion is not merely a service-level unit
/// test, it holds across the real HTTP → extractor → service → commit path.
#[tokio::test]
async fn a_kg_declared_weight_is_converted_over_http() {
    let (app, token, db) = app_with_ingest_token().await;
    let payload = serde_json::json!({ "data": { "metrics": [
        { "name": "weight_body_mass", "units": "kg",
          "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 100.0 }] }
    ] } });

    let resp = app
        .oneshot(ingest_req(Some(&token), &payload))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let rows = <healthie_shared::entities::daily_metric::Entity as sea_orm::EntityTrait>::find()
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        (rows[0].value - 220.462_262_184_877_6).abs() < 1e-9,
        "100 kg must be stored as pounds, got {}",
        rows[0].value
    );
}
