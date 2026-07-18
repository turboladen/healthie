//! Wire-level tests: drive the production `router()` through real JSON-RPC
//! over streamable HTTP via `tower::oneshot` — no network socket.

use axum::{Router, body::Body, http::Request};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn setup() -> (Router, DatabaseConnection) {
    let db = healthie_shared::test_support::test_db().await;
    (healthie_mcp::router(db.clone()), db)
}

async fn post_rpc(app: &Router, body: Value) -> String {
    let request = Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .expect("build request");
    let response = app.clone().oneshot(request).await.expect("send request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf8 body");
    assert!(status.is_success(), "rpc failed: {status} body={text}");
    text
}

async fn handshake(app: &Router) {
    let body = post_rpc(
        app,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "healthie-test", "version": "0.0.0" }
            }
        }),
    )
    .await;
    assert!(
        body.contains("healthie-mcp"),
        "initialize should identify the server: {body}"
    );
    post_rpc(
        app,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
}

// `Value` by value keeps the call sites ergonomic (`call_tool(n, json!({…}))`);
// serde_json's `json!` only borrows it, hence the needless-pass-by-value allow.
#[allow(dead_code, clippy::needless_pass_by_value)] // used from Task 6 on
fn call_tool(name: &str, args: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": name, "arguments": args }
    })
}

/// Parse a tools/call response body; returns (`is_error`, first text block).
#[allow(dead_code)] // used from Task 6 on
fn tool_result(body: &str) -> (bool, String) {
    let parsed: Value = serde_json::from_str(body).expect("json body");
    let result = parsed
        .get("result")
        .unwrap_or_else(|| panic!("no result in {body}"));
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    (is_error, text)
}

/// Successful tool call → payload parsed back from the pretty-JSON text block.
#[allow(dead_code)] // used from Task 6 on
fn tool_payload(body: &str) -> Value {
    let (is_error, text) = tool_result(body);
    assert!(!is_error, "tool errored: {text}");
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("payload not JSON: {text}"))
}

#[allow(dead_code)] // used from Task 7 on
fn assert_tool_error(body: &str, needle: &str) {
    let (is_error, text) = tool_result(body);
    assert!(is_error, "expected tool error, got success: {text}");
    assert!(
        text.contains(needle),
        "error should mention '{needle}': {text}"
    );
}

#[tokio::test]
async fn handshake_succeeds_statelessly() {
    let (app, _db) = setup().await;
    handshake(&app).await;
}

#[tokio::test]
async fn tools_list_responds() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert!(body.contains("\"tools\""), "{body}");
}
