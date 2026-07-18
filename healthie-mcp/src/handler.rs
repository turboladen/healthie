//! The MCP handler: state struct, tool router, and `ServerHandler` impl.
//! Tools are strictly schema-struct → `into_domain()` → shared-service call →
//! serialized result. All business logic and validation live in
//! `healthie-shared` — nothing here may validate domain rules.

use std::sync::Arc;

use healthie_shared::{
    clock,
    error::{DomainError, DomainResult},
    services::{briefing, concern, goal, observation, profile, protocol},
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

use crate::schemas::{
    EmptyParams, LogObservationInput, LogSymptomInput, OpenConcernInput,
    RecordProtocolOutcomeInput, SetGoalInput, StartProtocolInput, UpdateConcernStatusInput,
    UpdateProfileInput,
};

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

    #[tool(
        name = "open_concern",
        description = "Open a new health concern (the unit everything else hangs off: goals, \
            protocols, observations). Use when something new is worth tracking, not for \
            one-off observations — those are log_observation.",
        input_schema = rmcp::handler::server::common::schema_for_type::<OpenConcernInput>()
    )]
    async fn open_concern(
        &self,
        params: LenientParameters<OpenConcernInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("open_concern") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(concern::open(&*self.db, input.into_domain()).await)
    }

    #[tool(
        name = "update_concern_status",
        description = "Move a concern between active / monitoring / resolved, optionally \
            appending a dated note to its narrative. Resolving stamps resolved_on.",
        input_schema = rmcp::handler::server::common::schema_for_type::<UpdateConcernStatusInput>()
    )]
    async fn update_concern_status(
        &self,
        params: LenientParameters<UpdateConcernStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("update_concern_status") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(
            concern::update_status(&*self.db, input.concern_id, input.status, input.note).await,
        )
    }

    #[tool(
        name = "set_goal",
        description = "Set a goal, optionally under a concern. Measurable goals carry a \
            comparison (at-most / at-least / range) with target values.",
        input_schema = rmcp::handler::server::common::schema_for_type::<SetGoalInput>()
    )]
    async fn set_goal(
        &self,
        params: LenientParameters<SetGoalInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("set_goal") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(goal::set(&*self.db, input.into_domain()).await)
    }

    #[tool(
        name = "start_protocol",
        description = "Start a protocol: a deliberate intervention (supplement, exercise, \
            diet, therapy, habit, …) with a purpose and review-by date. Check \
            get_protocol_history first — verdicts are permanent so nothing is re-tried blind.",
        input_schema = rmcp::handler::server::common::schema_for_type::<StartProtocolInput>()
    )]
    async fn start_protocol(
        &self,
        params: LenientParameters<StartProtocolInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("start_protocol") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(protocol::start(&*self.db, input.into_domain()).await)
    }

    #[tool(
        name = "record_protocol_outcome",
        description = "Record a protocol's final verdict (worked / didnt-work / mixed / \
            inconclusive) with a mandatory rationale. Permanent — one verdict per protocol, \
            this is the record future planning consults.",
        input_schema = rmcp::handler::server::common::schema_for_type::<RecordProtocolOutcomeInput>()
    )]
    async fn record_protocol_outcome(
        &self,
        params: LenientParameters<RecordProtocolOutcomeInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("record_protocol_outcome") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        let (id, outcome) = input.into_domain();
        domain_result(protocol::record_outcome(&*self.db, id, outcome).await)
    }

    #[tool(
        name = "get_protocol_history",
        description = "Every protocol ever tried, most recent first, with verdicts and \
            rationales. Consult before proposing any new intervention.",
        input_schema = rmcp::handler::server::common::schema_for_type::<EmptyParams>()
    )]
    async fn get_protocol_history(
        &self,
        params: LenientParameters<EmptyParams>,
    ) -> Result<CallToolResult, McpError> {
        let EmptyParams {} = match params.into_tool_input("get_protocol_history") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(protocol::history(&*self.db).await)
    }

    #[tool(
        name = "update_profile",
        description = "Set profile facts (date of birth, sex, height, standing notes) that \
            every briefing carries. Set-only: omitted fields are untouched.",
        input_schema = rmcp::handler::server::common::schema_for_type::<UpdateProfileInput>()
    )]
    async fn update_profile(
        &self,
        params: LenientParameters<UpdateProfileInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("update_profile") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(profile::upsert(&*self.db, input.into_domain()).await)
    }

    #[tool(
        name = "log_observation",
        description = "Capture a health note the moment it comes up — sleep, mood, energy, \
            context. Anything worth the next briefing knowing. Not for symptoms \
            (log_symptom) or commitments (commit_plan).",
        input_schema = rmcp::handler::server::common::schema_for_type::<LogObservationInput>()
    )]
    async fn log_observation(
        &self,
        params: LenientParameters<LogObservationInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("log_observation") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(observation::log(&*self.db, input.into_domain()).await)
    }

    #[tool(
        name = "log_symptom",
        description = "Record a symptom with optional 1-10 severity, linked to a concern \
            when clearly related. Recurring symptoms may deserve open_concern.",
        input_schema = rmcp::handler::server::common::schema_for_type::<LogSymptomInput>()
    )]
    async fn log_symptom(
        &self,
        params: LenientParameters<LogSymptomInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("log_symptom") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(observation::log(&*self.db, input.into_domain()).await)
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
