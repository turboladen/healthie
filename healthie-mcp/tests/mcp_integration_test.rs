//! Wire-level tests: drive the production `router()` through real JSON-RPC
//! over streamable HTTP via `tower::oneshot` — no network socket.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn setup() -> (Router, DatabaseConnection, String) {
    let db = healthie_shared::test_support::test_db().await;
    let issued = healthie_shared::services::mcp_token::provision(&db)
        .await
        .expect("provision test token");
    (healthie_mcp::router(db.clone()), db, issued.plaintext)
}

async fn post_rpc_as(app: &Router, bearer: Option<&str>, body: Value) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = builder
        .body(Body::from(body.to_string()))
        .expect("build request");
    let response = app.clone().oneshot(request).await.expect("send request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    (
        status,
        String::from_utf8(bytes.to_vec()).expect("utf8 body"),
    )
}

async fn post_rpc(app: &Router, token: &str, body: Value) -> String {
    let (status, text) = post_rpc_as(app, Some(token), body).await;
    assert!(status.is_success(), "rpc failed: {status} body={text}");
    text
}

async fn handshake(app: &Router, token: &str) {
    let body = post_rpc(
        app,
        token,
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
        token,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
}

// `Value` by value keeps the call sites ergonomic (`call_tool(n, json!({…}))`);
// serde_json's `json!` only borrows it, hence the needless-pass-by-value allow.
#[allow(clippy::needless_pass_by_value)]
fn call_tool(name: &str, args: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": name, "arguments": args }
    })
}

/// Parse a tools/call response body; returns (`is_error`, first text block).
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
fn tool_payload(body: &str) -> Value {
    let (is_error, text) = tool_result(body);
    assert!(!is_error, "tool errored: {text}");
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("payload not JSON: {text}"))
}

fn assert_tool_error(body: &str, needle: &str) {
    let (is_error, text) = tool_result(body);
    assert!(is_error, "expected tool error, got success: {text}");
    assert!(
        text.contains(needle),
        "error should mention '{needle}': {text}"
    );
}

#[tokio::test]
async fn requests_without_bearer_are_401() {
    let (app, _db, _token) = setup().await;
    let (status, body) = post_rpc_as(
        &app,
        None,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body.contains("Bearer"),
        "401 body should say what's missing: {body}"
    );
    let parsed: Value = serde_json::from_str(&body).expect("401 body is json");
    assert!(
        parsed["error"].is_string(),
        "401 must carry the {{\"error\": ...}} contract: {body}"
    );
}

#[tokio::test]
async fn unrecognized_token_is_401_before_rmcp() {
    let (app, _db, _token) = setup().await;
    let shaped_but_wrong = "A".repeat(43);
    let (status, body) = post_rpc_as(
        &app,
        Some(&shaped_but_wrong),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("invalid or revoked"), "{body}");
}

#[tokio::test]
async fn revoked_and_rotated_tokens_are_401() {
    let (app, db, token) = setup().await;
    healthie_shared::services::mcp_token::revoke(&db)
        .await
        .expect("revoke");
    let (status, _) = post_rpc_as(
        &app,
        Some(&token),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked token must fail");

    let second = healthie_shared::services::mcp_token::provision(&db)
        .await
        .expect("rotate");
    let (status, _) = post_rpc_as(
        &app,
        Some(&token),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "pre-rotation token must fail"
    );
    let (status, _) = post_rpc_as(
        &app,
        Some(&second.plaintext),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await;
    assert!(status.is_success(), "current token must work");
}

#[tokio::test]
async fn valid_token_reaches_tools_end_to_end() {
    // Proves middleware → axum Parts → rmcp RequestContext → tool guard plumbing.
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(&app, &token, call_tool("get_briefing", json!({}))).await;
    let payload = tool_payload(&body);
    assert!(payload["generated_on"].is_string());
    assert!(!body.contains("missing authenticated operator"), "{body}");
}

#[tokio::test]
async fn unknown_host_header_is_rejected() {
    // rmcp's DNS-rebinding defense stays live behind our auth layer.
    let (app, _db, token) = setup().await;
    let request = Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "evil.example.com")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
        ))
        .expect("build request");
    let response = app.clone().oneshot(request).await.expect("send");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn handshake_succeeds_statelessly() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
}

#[tokio::test]
async fn get_briefing_on_empty_db_returns_briefing_json() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(&app, &token, call_tool("get_briefing", json!({}))).await;
    let payload = tool_payload(&body);
    assert!(payload["generated_on"].is_string());
    assert_eq!(payload["active_concerns"], json!([]));
    assert!(payload["last_checkin"].is_null());
}

#[tokio::test]
async fn open_concern_round_trips_with_tags() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
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

    let body = post_rpc(&app, &token, call_tool("get_briefing", json!({}))).await;
    let payload = tool_payload(&body);
    assert_eq!(
        payload["active_concerns"][0]["concern"]["name"],
        "Right shoulder pain"
    );
}

#[tokio::test]
async fn open_concern_rejects_unknown_tag_with_schema_hint() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
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
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
        call_tool("open_concern", json!({ "name": "Sleep debt" })),
    )
    .await;
    let id = tool_payload(&body)["concern"]["id"].as_i64().expect("id");

    let body = post_rpc(
        &app,
        &token,
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
        &token,
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
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    // range comparison without target_high is a domain Invalid
    let body = post_rpc(
        &app,
        &token,
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
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
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
        &token,
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
        &token,
        call_tool(
            "record_protocol_outcome",
            json!({ "protocol_id": id, "verdict": "mixed", "rationale": "second thoughts" }),
        ),
    )
    .await;
    let (is_error, _) = tool_result(&body);
    assert!(is_error, "verdict must be immutable");

    let body = post_rpc(&app, &token, call_tool("get_protocol_history", json!({}))).await;
    let payload = tool_payload(&body);
    assert_eq!(payload[0]["name"], "Creatine 5g daily");
    assert_eq!(payload[0]["verdict"], "worked");
}

#[tokio::test]
async fn update_profile_sets_fields_visible_in_briefing() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "update_profile",
            json!({ "date_of_birth": "1980-03-14", "sex": "male", "height_cm": 180 }),
        ),
    )
    .await;
    assert_eq!(tool_payload(&body)["height_cm"], 180);

    let body = post_rpc(&app, &token, call_tool("get_briefing", json!({}))).await;
    assert_eq!(tool_payload(&body)["profile"]["sex"], "male");
}

#[tokio::test]
async fn log_observation_from_self_is_auto_reviewed() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
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
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
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
        &token,
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
async fn full_checkin_loop_start_respond_plan_complete_outcome() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;

    let body = post_rpc(&app, &token, call_tool("start_checkin", json!({}))).await;
    let checkin_id = tool_payload(&body)["id"].as_i64().expect("checkin id");

    // start_checkin is idempotent within a day — resuming returns the same id.
    let body = post_rpc(&app, &token, call_tool("start_checkin", json!({}))).await;
    assert_eq!(tool_payload(&body)["id"].as_i64(), Some(checkin_id));

    for (q, a) in [
        ("How was sleep since last time?", "Rough — averaging 6h."),
        (
            "Any pain flare-ups?",
            "Shoulder acted up twice after climbing.",
        ),
    ] {
        let body = post_rpc(
            &app,
            &token,
            call_tool(
                "record_checkin_response",
                json!({ "checkin_id": checkin_id, "question": q, "answer": a }),
            ),
        )
        .await;
        assert_eq!(tool_payload(&body)["question"], q);
    }

    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "commit_plan",
            json!({
                "checkin_id": checkin_id,
                "guidance": "Protect sleep before volume.",
                "items": [
                    { "kind": "workout", "title": "PT shoulder rotation", "scheduled_for": "2026-07-20" },
                    { "kind": "action", "title": "Book dentist cleaning" }
                ]
            }),
        ),
    )
    .await;
    let plan = tool_payload(&body);
    let item_id = plan["items"][0]["item"]["id"].as_i64().expect("item id");
    assert_eq!(plan["items"][1]["item"]["kind"], "action");

    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "complete_checkin",
            json!({
                "checkin_id": checkin_id,
                "summary": "Sleep-first week; shoulder PT recommitted; dentist booking pending."
            }),
        ),
    )
    .await;
    assert!(tool_payload(&body)["completed_at"].is_string());

    // Completing twice is a BadRequest tool error.
    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "complete_checkin",
            json!({ "checkin_id": checkin_id, "summary": "again" }),
        ),
    )
    .await;
    let (is_error, _) = tool_result(&body);
    assert!(is_error, "double-complete must be a tool error");

    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "record_plan_outcome",
            json!({ "plan_item_id": item_id, "status": "done", "note": "All 3 sessions." }),
        ),
    )
    .await;
    assert_eq!(tool_payload(&body)["status"], "done");

    // The next briefing opens with the commitments + outcomes on file.
    let body = post_rpc(&app, &token, call_tool("get_briefing", json!({}))).await;
    let briefing = tool_payload(&body);
    assert_eq!(
        briefing["last_checkin"]["summary"],
        "Sleep-first week; shoulder PT recommitted; dentist booking pending."
    );
    assert_eq!(
        briefing["previous_plan"]["items"][0]["outcome"]["status"],
        "done"
    );
}

#[tokio::test]
async fn record_checkin_response_rejects_completed_checkin() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(&app, &token, call_tool("start_checkin", json!({}))).await;
    let id = tool_payload(&body)["id"].as_i64().expect("id");
    post_rpc(
        &app,
        &token,
        call_tool(
            "complete_checkin",
            json!({ "checkin_id": id, "summary": "quick one" }),
        ),
    )
    .await;
    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "record_checkin_response",
            json!({ "checkin_id": id, "question": "late?", "answer": "too late" }),
        ),
    )
    .await;
    let (is_error, _) = tool_result(&body);
    assert!(is_error, "responses after completion must be rejected");
}

#[tokio::test]
async fn record_checkin_response_links_a_real_concern() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
        call_tool("open_concern", json!({ "name": "Lower back tightness" })),
    )
    .await;
    let concern_id = tool_payload(&body)["concern"]["id"]
        .as_i64()
        .expect("concern id");

    let body = post_rpc(&app, &token, call_tool("start_checkin", json!({}))).await;
    let checkin_id = tool_payload(&body)["id"].as_i64().expect("checkin id");

    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "record_checkin_response",
            json!({
                "checkin_id": checkin_id,
                "question": "How's the back?",
                "answer": "Looser after the mobility work.",
                "concern_id": concern_id
            }),
        ),
    )
    .await;
    let payload = tool_payload(&body);
    assert_eq!(payload["concern_id"].as_i64(), Some(concern_id));
}

#[tokio::test]
async fn record_checkin_response_rejects_unknown_concern() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(&app, &token, call_tool("start_checkin", json!({}))).await;
    let checkin_id = tool_payload(&body)["id"].as_i64().expect("checkin id");

    let body = post_rpc(
        &app,
        &token,
        call_tool(
            "record_checkin_response",
            json!({
                "checkin_id": checkin_id,
                "question": "Anything about the shoulder?",
                "answer": "No change.",
                "concern_id": 9999
            }),
        ),
    )
    .await;
    assert_tool_error(&body, "not found");
}

#[tokio::test]
async fn commit_plan_requires_items() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
        call_tool("commit_plan", json!({ "items": [] })),
    )
    .await;
    let (is_error, _) = tool_result(&body);
    assert!(is_error, "empty plan must be rejected");
}

#[tokio::test]
async fn briefing_resource_lists_and_reads() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;

    let body = post_rpc(
        &app,
        &token,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }),
    )
    .await;
    assert!(body.contains("healthie://briefing"), "{body}");
    assert!(
        body.contains("application/json"),
        "resource entry must advertise its mime type: {body}"
    );

    let body = post_rpc(
        &app,
        &token,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "resources/read",
            "params": { "uri": "healthie://briefing" }
        }),
    )
    .await;
    let parsed: Value = serde_json::from_str(&body).expect("json");
    let text = parsed["result"]["contents"][0]["text"]
        .as_str()
        .expect("text contents");
    let briefing: Value = serde_json::from_str(text).expect("briefing json");
    assert!(briefing["generated_on"].is_string());
}

#[tokio::test]
async fn unknown_resource_uri_is_invalid_params() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "resources/read",
            "params": { "uri": "healthie://nope" }
        }),
    )
    .await;
    assert!(body.contains("error"), "{body}");
    assert!(
        body.contains("healthie://briefing"),
        "error should list known URIs: {body}"
    );
}

#[tokio::test]
async fn checkin_prompt_renders_over_the_wire() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "prompts/get",
            "params": { "name": "checkin", "arguments": { "focus": "knee clicking" } }
        }),
    )
    .await;
    assert!(body.contains("knee clicking"), "{body}");
    assert!(body.contains("get_briefing"), "{body}");
}

/// The 15 tools the M1b surface must advertise.
const EXPECTED_TOOLS: [&str; 15] = [
    "get_briefing",
    "start_checkin",
    "record_checkin_response",
    "complete_checkin",
    "commit_plan",
    "record_plan_outcome",
    "log_observation",
    "log_symptom",
    "open_concern",
    "update_concern_status",
    "set_goal",
    "start_protocol",
    "record_protocol_outcome",
    "get_protocol_history",
    "update_profile",
];

#[tokio::test]
async fn tools_list_advertises_all_tools_with_populated_schemas() {
    let (app, _db, token) = setup().await;
    handshake(&app, &token).await;
    let body = post_rpc(
        &app,
        &token,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    let parsed: Value = serde_json::from_str(&body).expect("json body");
    let tools = parsed["result"]["tools"].as_array().expect("tools array");

    // (2) Every expected tool is registered — catches a macro registration slip.
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in EXPECTED_TOOLS {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }
    assert_eq!(
        names.len(),
        EXPECTED_TOOLS.len(),
        "unexpected tool count: {names:?}"
    );

    // (1) An arg-bearing tool must advertise a non-empty inputSchema — guards
    // the per-tool `input_schema = schema_for_type::<T>()` override footgun.
    let open_concern = tools
        .iter()
        .find(|t| t["name"] == "open_concern")
        .expect("open_concern present");
    let properties = open_concern["inputSchema"]["properties"]
        .as_object()
        .expect("open_concern inputSchema.properties object");
    assert!(
        properties.contains_key("name") && properties.contains_key("tags"),
        "open_concern schema must expose its fields: {}",
        open_concern["inputSchema"]
    );
    // The tags enum vocabulary must ride straight from the domain type
    // (schemars feature end-to-end), not a hand-mirrored string list.
    assert!(
        body.contains("musculoskeletal"),
        "ConcernTag vocab must appear in the advertised schema: {body}"
    );

    // No-argument tools correctly advertise an empty (property-less) schema.
    let get_briefing = tools
        .iter()
        .find(|t| t["name"] == "get_briefing")
        .expect("get_briefing present");
    let empty_props = get_briefing["inputSchema"]["properties"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    assert_eq!(empty_props, 0, "EmptyParams tool must have no properties");
}
