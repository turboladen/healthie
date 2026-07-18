//! healthie-mcp — the MCP surface over `healthie-shared` (ADR-0002 M1b).
//!
//! A library exposing [`router`] (nested by the M2 backend at `/mcp`) plus a
//! binary (`healthie-mcp serve|token`) that hosts it until the backend exists.
//! Transport is streamable HTTP in STATELESS mode: no session ids, plain JSON
//! responses — server restarts never strand a client mid-conversation.
//! Security: bearer-token middleware (added in a later unit) + rmcp's Host
//! allowlist (DNS-rebinding defense; extend via `HEALTHIE_MCP_ALLOWED_HOSTS`).

mod handler;
mod schemas;

use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use sea_orm::DatabaseConnection;

pub use handler::HealthieMcp;

/// Build the `/mcp` router: stateless streamable-HTTP rmcp service.
pub fn router(db: DatabaseConnection) -> Router {
    let default_config = StreamableHttpServerConfig::default();
    let allowed_hosts = merge_allowed_hosts(
        default_config.allowed_hosts.clone(),
        std::env::var("HEALTHIE_MCP_ALLOWED_HOSTS").ok().as_deref(),
    );
    // Stateless: no sessions to strand on restart, and plain-JSON responses
    // instead of SSE — simpler for HTTP-only bridges.
    let mut http_config = default_config.with_allowed_hosts(allowed_hosts);
    http_config.stateful_mode = false;
    http_config.json_response = true;

    let streamable = StreamableHttpService::new(
        move || Ok(HealthieMcp::new(db.clone())),
        // Required by the constructor; unused when stateful_mode is false.
        Arc::new(LocalSessionManager::default()),
        http_config,
    );
    Router::new().fallback_service(streamable)
}

/// Append operator hosts from the env var to rmcp's own localhost defaults —
/// pulled from rmcp's config at runtime so an rmcp upgrade can't silently
/// drift the list. Comma-separated; blank entries skipped; an entry without a
/// port matches any port.
fn merge_allowed_hosts(defaults: Vec<String>, env_value: Option<&str>) -> Vec<String> {
    let mut hosts = defaults;
    if let Some(raw) = env_value {
        for entry in raw.split(',') {
            let trimmed = entry.trim();
            if !trimmed.is_empty() {
                hosts.push(trimmed.to_string());
            }
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_env_hosts_to_defaults() {
        let merged = merge_allowed_hosts(
            vec!["localhost".into()],
            Some("odroid.tailnet.ts.net, dietpi.local:3005,, "),
        );
        assert_eq!(
            merged,
            vec!["localhost", "odroid.tailnet.ts.net", "dietpi.local:3005"]
        );
    }

    #[test]
    fn merge_without_env_is_defaults_verbatim() {
        let defaults = StreamableHttpServerConfig::default().allowed_hosts;
        let merged = merge_allowed_hosts(defaults.clone(), None);
        assert_eq!(merged, defaults);
    }
}
