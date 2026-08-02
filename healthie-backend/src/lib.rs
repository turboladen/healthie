//! healthie-backend — the single deployed binary (ADR-0005): the REST API,
//! `POST /ingest/hae`, and the MCP router mounted at `/mcp`. Exposed as a
//! library (alongside the `healthie-backend` binary) so wire tests can drive
//! [`api::router`] through `tower::oneshot` without binding a socket.

pub mod api;
pub mod apple_health;
pub mod config;

use sea_orm::DatabaseConnection;

/// Shared handler state. Holds only the database connection for M2; it grows as
/// handlers gain dependencies.
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
}
