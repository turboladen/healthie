//! The MCP handler: state struct, tool router, and `ServerHandler` impl.
//! Tools are strictly schema-struct → `into_domain()` → shared-service call →
//! serialized result. All business logic and validation live in
//! `healthie-shared` — nothing here may validate domain rules.

use std::sync::Arc;

use healthie_shared::{
    clock,
    error::{DomainError, DomainResult},
    services::briefing,
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{common::FromContextPart, tool::ToolCallContext},
    model::{
        CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo,
        ToolsCapability,
    },
    tool, tool_handler, tool_router,
};
use sea_orm::DatabaseConnection;
use serde::{Serialize, de::DeserializeOwned};

use crate::schemas::EmptyParams;

#[derive(Clone)]
pub struct HealthieMcp {
    db: Arc<DatabaseConnection>,
}

#[tool_router]
impl HealthieMcp {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db: Arc::new(db) }
    }

    #[tool(
        name = "get_briefing",
        description = "Start here every conversation: the current health briefing — profile, \
            days since last checkin, the last checkin's summary and responses, the previous \
            plan with per-item outcomes, active concerns/goals/protocols (with overdue review \
            flags), and observations pending review. Date-aware: after a long gap it widens \
            its windows and says so.",
        input_schema = rmcp::handler::server::common::schema_for_type::<EmptyParams>()
    )]
    async fn get_briefing(
        &self,
        params: LenientParameters<EmptyParams>,
    ) -> Result<CallToolResult, McpError> {
        let EmptyParams {} = match params.into_tool_input("get_briefing") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(briefing::assemble(&*self.db, clock::today()).await)
    }
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

// ---------- shared tool plumbing ----------

/// Argument wrapper replacing rmcp's `Parameters<T>`: a deserialize failure
/// becomes a tool-level error RESULT carrying the schema hint, instead of a
/// bare JSON-RPC -32602 most clients render as an unexplained failure.
///
/// GOTCHA (from glovebox): because the signature doesn't literally use
/// `Parameters<T>`, rmcp's macro can NOT auto-derive the input schema — every
/// `#[tool]` must pass
/// `input_schema = rmcp::handler::server::common::schema_for_type::<T>()`
/// or it silently advertises an empty schema.
pub(crate) struct LenientParameters<T>(Result<T, String>);

impl<T> LenientParameters<T> {
    pub(crate) fn into_tool_input(self, tool_name: &'static str) -> Result<T, CallToolResult> {
        self.0.map_err(|e| {
            tool_user_error(format!("{tool_name}: {e}. Check the tool's input schema."))
        })
    }
}

impl<S, T> FromContextPart<ToolCallContext<'_, S>> for LenientParameters<T>
where
    T: DeserializeOwned,
{
    fn from_context_part(context: &mut ToolCallContext<S>) -> Result<Self, McpError> {
        let arguments = context.arguments.take().unwrap_or_default();
        let parsed = serde_json::from_value::<T>(serde_json::Value::Object(arguments))
            .map_err(|e| e.to_string());
        Ok(Self(parsed))
    }
}

/// The single canonical `DomainError` → MCP mapping. Recoverable variants
/// become tool-level error results (the actionable message reaches the LLM);
/// `Db`/`Internal` are deliberately opaque protocol errors with detail kept
/// server-side in tracing.
fn domain_error(err: DomainError) -> Result<CallToolResult, McpError> {
    match err {
        DomainError::NotFound(_) | DomainError::Invalid { .. } | DomainError::BadRequest(_) => {
            Ok(tool_user_error(err.to_string()))
        }
        DomainError::Db(db_err) => {
            tracing::error!(?db_err, "MCP tool: database error");
            Err(McpError::internal_error("database error", None))
        }
        DomainError::Internal(detail) => {
            tracing::error!(detail, "MCP tool: internal error");
            Err(McpError::internal_error("internal error", None))
        }
    }
}

/// Serialize a service result or route its error through [`domain_error`].
fn domain_result<T: Serialize>(result: DomainResult<T>) -> Result<CallToolResult, McpError> {
    match result {
        Ok(value) => tool_json_result(&value),
        Err(err) => domain_error(err),
    }
}

fn tool_json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value).map_err(|err| {
        tracing::error!(?err, "MCP tool: result serialization failed");
        McpError::internal_error("serialization error", None)
    })?;
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

/// Build a tool-level error result so the actionable message reaches the LLM
/// (a protocol-level `Err(McpError)` is rendered by most clients as a generic
/// "tool failed" with the message dropped).
fn tool_user_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}
