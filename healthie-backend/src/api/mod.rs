//! HTTP surface. The `/api/health` + `/ingest/hae` routes run under permissive
//! CORS; the MCP router is mounted OUTSIDE CORS so browsers cannot drive its
//! tools. The bearer(ingest) auth layer on `/ingest/hae` lands in Task 9 — until
//! then no route is gated (health must never sit behind ingest auth).

pub mod health;
pub mod ingest;

use axum::{
    Router,
    routing::{get, post},
};
use sea_orm::DatabaseConnection;
use tower_http::cors::CorsLayer;

use crate::AppState;

/// Assemble the whole application router. `mcp_db` is a distinct connection
/// handle moved into the nested MCP router.
pub fn router(state: AppState, mcp_db: DatabaseConnection) -> Router {
    Router::new()
        .route("/api/health", get(health::check))
        .route("/ingest/hae", post(ingest::hae))
        .layer(CorsLayer::permissive())
        // Mounted AFTER CORS on purpose: MCP clients aren't browsers, and
        // permissive CORS here would let any web page a user visits drive the
        // tools from their browser. See healthie-mcp's crate docs.
        .nest_service("/mcp", healthie_mcp::router(mcp_db))
        .with_state(state)
}
