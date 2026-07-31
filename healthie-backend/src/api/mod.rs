//! HTTP surface. `/api/health` and `/ingest/hae` run under permissive CORS; the
//! MCP router is mounted OUTSIDE CORS so browsers cannot drive its tools. The
//! bearer(ingest) gate is route-scoped to `/ingest/hae` alone, so `/api/health`
//! is never behind it.

pub mod error;
pub mod health;
pub mod ingest;

use axum::{
    Router, middleware,
    routing::{get, post},
};
use sea_orm::DatabaseConnection;
use tower_http::cors::CorsLayer;

use crate::AppState;

/// Assemble the whole application router. `mcp_db` is a distinct connection
/// handle moved into the nested MCP router.
pub fn router(state: AppState, mcp_db: DatabaseConnection) -> Router {
    // Ingest is its own sub-router so `route_layer` scopes the bearer(ingest)
    // gate to ONLY `/ingest/hae` — `/api/health` (added on the outer router)
    // is never behind it. `route_layer` also skips unmatched paths.
    let ingest = Router::new()
        .route("/ingest/hae", post(ingest::hae))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            ingest::require_ingest_token,
        ));

    Router::new()
        .route("/api/health", get(health::check))
        .merge(ingest)
        .layer(CorsLayer::permissive())
        // Mounted AFTER CORS on purpose: MCP clients aren't browsers, and
        // permissive CORS here would let any web page a user visits drive the
        // tools from their browser. See healthie-mcp's crate docs.
        .nest_service("/mcp", healthie_mcp::router(mcp_db))
        .with_state(state)
}
