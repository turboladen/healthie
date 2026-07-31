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
