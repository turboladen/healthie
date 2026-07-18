//! The MCP handler: state struct, tool router, and `ServerHandler` impl.
//! Tools are strictly schema-struct → `into_domain()` → shared-service call →
//! serialized result. All business logic and validation live in
//! `healthie-shared` — nothing here may validate domain rules.

use std::sync::Arc;

use rmcp::{
    ServerHandler,
    model::{Implementation, ServerCapabilities, ServerInfo, ToolsCapability},
    tool_handler, tool_router,
};
use sea_orm::DatabaseConnection;

#[derive(Clone)]
pub struct HealthieMcp {
    #[allow(dead_code)] // first read by the tools added in Task 6
    db: Arc<DatabaseConnection>,
}

#[tool_router]
impl HealthieMcp {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db: Arc::new(db) }
    }
    // Tools land here in Tasks 6–9.
}

#[tool_handler]
impl ServerHandler for HealthieMcp {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::default();
        capabilities.tools = Some(ToolsCapability::default());
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new(
                "healthie-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions("healthie MCP: placeholder — finalized in the wrap-up task.")
    }
}
