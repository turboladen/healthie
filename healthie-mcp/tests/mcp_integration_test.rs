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
async fn get_briefing_on_empty_db_returns_briefing_json() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(&app, call_tool("get_briefing", json!({}))).await;
    let payload = tool_payload(&body);
    assert!(payload["generated_on"].is_string());
    assert_eq!(payload["active_concerns"], json!([]));
    assert!(payload["last_checkin"].is_null());
}

#[tokio::test]
async fn open_concern_round_trips_with_tags() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        call_tool(
            "open_concern",
            json!({
                "name": "Right shoulder pain",
                "narrative": "Started after deadlifts in June.",
                "tags": ["musculoskeletal", "fitness"]
            }),
        ),
    )
    .await;
    let payload = tool_payload(&body);
    assert_eq!(payload["concern"]["name"], "Right shoulder pain");
    assert_eq!(payload["tags"], json!(["musculoskeletal", "fitness"]));

    let body = post_rpc(&app, call_tool("get_briefing", json!({}))).await;
    let payload = tool_payload(&body);
    assert_eq!(
        payload["active_concerns"][0]["concern"]["name"],
        "Right shoulder pain"
    );
}

#[tokio::test]
async fn open_concern_rejects_unknown_tag_with_schema_hint() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        call_tool(
            "open_concern",
            json!({ "name": "X", "tags": ["not-a-real-tag"] }),
        ),
    )
    .await;
    assert_tool_error(&body, "input schema");
}

#[tokio::test]
async fn update_concern_status_resolves_and_reports_not_found() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        call_tool("open_concern", json!({ "name": "Sleep debt" })),
    )
    .await;
    let id = tool_payload(&body)["concern"]["id"].as_i64().expect("id");

    let body = post_rpc(
        &app,
        call_tool(
            "update_concern_status",
            json!({ "concern_id": id, "status": "resolved", "note": "Consistent 7h for a month." }),
        ),
    )
    .await;
    let payload = tool_payload(&body);
    assert_eq!(payload["status"], "resolved");
    assert!(payload["resolved_on"].is_string());

    let body = post_rpc(
        &app,
        call_tool(
            "update_concern_status",
            json!({ "concern_id": 9999, "status": "active" }),
        ),
    )
    .await;
    assert_tool_error(&body, "not found");
}

#[tokio::test]
async fn set_goal_surfaces_domain_validation_as_tool_error() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    // range comparison without target_high is a domain Invalid
    let body = post_rpc(
        &app,
        call_tool(
            "set_goal",
            json!({ "title": "Bodyweight in range", "comparison": "range", "target_value": 175.0 }),
        ),
    )
    .await;
    let (is_error, text) = tool_result(&body);
    assert!(is_error, "expected domain validation to surface: {text}");
}

#[tokio::test]
async fn protocol_lifecycle_start_outcome_history() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        call_tool(
            "start_protocol",
            json!({
                "name": "Creatine 5g daily", "kind": "supplement",
                "purpose": "Strength + cognition support", "review_by": "2026-10-01"
            }),
        ),
    )
    .await;
    let id = tool_payload(&body)["id"].as_i64().expect("id");

    let body = post_rpc(
        &app,
        call_tool(
            "record_protocol_outcome",
            json!({ "protocol_id": id, "verdict": "worked", "rationale": "Gym volume up, no sides." }),
        ),
    )
    .await;
    assert_eq!(tool_payload(&body)["verdict"], "worked");

    // Verdicts are permanent — recording twice is a BadRequest tool error.
    let body = post_rpc(
        &app,
        call_tool(
            "record_protocol_outcome",
            json!({ "protocol_id": id, "verdict": "mixed", "rationale": "second thoughts" }),
        ),
    )
    .await;
    let (is_error, _) = tool_result(&body);
    assert!(is_error, "verdict must be immutable");

    let body = post_rpc(&app, call_tool("get_protocol_history", json!({}))).await;
    let payload = tool_payload(&body);
    assert_eq!(payload[0]["name"], "Creatine 5g daily");
    assert_eq!(payload[0]["verdict"], "worked");
}

#[tokio::test]
async fn update_profile_sets_fields_visible_in_briefing() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        call_tool(
            "update_profile",
            json!({ "date_of_birth": "1980-03-14", "sex": "male", "height_cm": 180 }),
        ),
    )
    .await;
    assert_eq!(tool_payload(&body)["height_cm"], 180);

    let body = post_rpc(&app, call_tool("get_briefing", json!({}))).await;
    assert_eq!(tool_payload(&body)["profile"]["sex"], "male");
}

#[tokio::test]
async fn log_observation_from_self_is_auto_reviewed() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        call_tool(
            "log_observation",
            json!({ "origin": "self", "body": "Slept 8h, felt great." }),
        ),
    )
    .await;
    let payload = tool_payload(&body);
    assert_eq!(payload["kind"], "note");
    assert_eq!(payload["reviewed"], 1, "self-origin auto-reviewed");
}

#[tokio::test]
async fn log_symptom_validates_severity_range() {
    let (app, _db) = setup().await;
    handshake(&app).await;
    let body = post_rpc(
        &app,
        call_tool(
            "log_symptom",
            json!({ "origin": "self", "body": "Sharp shoulder twinge", "severity": 11 }),
        ),
    )
    .await;
    let (is_error, text) = tool_result(&body);
    assert!(is_error, "severity 11 must be rejected: {text}");

    let body = post_rpc(
        &app,
        call_tool(
            "log_symptom",
            json!({
                "origin": "ai", "body": "Recurring afternoon headaches inferred from notes",
                "severity": 4
            }),
        ),
    )
    .await;
    let payload = tool_payload(&body);
    assert_eq!(payload["kind"], "symptom");
    assert_eq!(payload["reviewed"], 0, "ai-origin awaits review");
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
