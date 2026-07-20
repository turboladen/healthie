//! The MCP handler: state struct, tool router, and `ServerHandler` impl.
//! Tools are strictly schema-struct → `into_domain()` → shared-service call →
//! serialized result. All business logic and validation live in
//! `healthie-shared` — nothing here may validate domain rules.

use std::sync::Arc;

use healthie_shared::{
    clock,
    error::{DomainError, DomainResult},
    services::{briefing, checkin, claim, concern, goal, observation, plan, profile, protocol},
};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{common::FromContextPart, tool::ToolCallContext},
    model::{
        CallToolResult, ContentBlock, Implementation, ListResourcesResult, PaginatedRequestParams,
        PromptsCapability, ReadResourceRequestParams, ReadResourceResult, Resource,
        ResourceContents, ResourcesCapability, ServerCapabilities, ServerInfo, ToolsCapability,
    },
    prompt_handler,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use sea_orm::DatabaseConnection;
use serde::{Serialize, de::DeserializeOwned};

use crate::schemas::{
    ClaimInput, CommitPlanInput, CompleteCheckinInput, EmptyParams, GetClaimsInput,
    LogObservationInput, LogSymptomInput, OpenConcernInput, RecordCheckinResponseInput,
    RecordIntakeAnswersInput, RecordPlanOutcomeInput, RecordProtocolOutcomeInput, SetGoalInput,
    StartProtocolInput, UpdateClaimInput, UpdateConcernStatusInput, UpdateProfileInput,
};

/// The one M1b resource. Per-concern dossiers and goal progress are follow-ups
/// (they need list-by-concern service fns / M2 `DailyMetrics`).
pub const BRIEFING_URI: &str = "healthie://briefing";

#[derive(Clone)]
pub struct HealthieMcp {
    db: Arc<DatabaseConnection>,
}

/// The `run_baseline_intake` payload: per-category coverage plus registry-wide
/// totals, all derived from the claims themselves (the sessionless intake
/// state, ADR-0004).
#[derive(Serialize)]
struct IntakeState {
    coverage: Vec<claim::CategoryCoverage>,
    total_claims: usize,
    total_unknowns: usize,
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Fails loudly if the router was ever mounted without the auth layer.
        let _operator = crate::auth::authenticated_operator(&context)?;
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
        name = "start_checkin",
        description = "Open (or resume) today's checkin. Idempotent within a day — calling \
            again returns the same open checkin with nothing lost. Call get_briefing first.",
        input_schema = rmcp::handler::server::common::schema_for_type::<EmptyParams>()
    )]
    async fn start_checkin(
        &self,
        params: LenientParameters<EmptyParams>,
    ) -> Result<CallToolResult, McpError> {
        let EmptyParams {} = match params.into_tool_input("start_checkin") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(checkin::start(&*self.db).await)
    }

    #[tool(
        name = "record_checkin_response",
        description = "Persist one question/answer exchange of the checkin, as it happens — \
            not in a batch at the end. Append-only; a dropped conversation stays resumable.",
        input_schema = rmcp::handler::server::common::schema_for_type::<RecordCheckinResponseInput>()
    )]
    async fn record_checkin_response(
        &self,
        params: LenientParameters<RecordCheckinResponseInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("record_checkin_response") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(
            checkin::record_response(
                &*self.db,
                input.checkin_id,
                &input.question,
                &input.answer,
                input.concern_id,
            )
            .await,
        )
    }

    #[tool(
        name = "complete_checkin",
        description = "Close the checkin with a summary. Write the summary for the NEXT \
            checkin's opening accountability pass — commitments made, state of play, \
            anything that must not be forgotten.",
        input_schema = rmcp::handler::server::common::schema_for_type::<CompleteCheckinInput>()
    )]
    async fn complete_checkin(
        &self,
        params: LenientParameters<CompleteCheckinInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("complete_checkin") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(checkin::complete(&*self.db, input.checkin_id, &input.summary).await)
    }

    #[tool(
        name = "commit_plan",
        description = "Commit the plan agreed in conversation: typed items (workout → \
            calendar-bound, action → discrete to-do) plus guidance/nutrition direction. \
            Healthie's copy is the source of truth; push items to external destinations \
            afterwards at the conversation layer.",
        input_schema = rmcp::handler::server::common::schema_for_type::<CommitPlanInput>()
    )]
    async fn commit_plan(
        &self,
        params: LenientParameters<CommitPlanInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("commit_plan") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(plan::commit(&*self.db, input.into_domain()).await)
    }

    #[tool(
        name = "record_plan_outcome",
        description = "Record what actually happened to a previous plan item: done / \
            skipped / partial, with a note for context. The heart of the accountability \
            loop — do this near the top of every checkin.",
        input_schema = rmcp::handler::server::common::schema_for_type::<RecordPlanOutcomeInput>()
    )]
    async fn record_plan_outcome(
        &self,
        params: LenientParameters<RecordPlanOutcomeInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("record_plan_outcome") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(
            plan::record_item_outcome(&*self.db, input.plan_item_id, input.status, input.note)
                .await,
        )
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

    #[tool(
        name = "run_baseline_intake",
        description = "Orient the baseline intake: per-category coverage of the claims \
            registry (claim count, unknowns to resolve, last touched — zero-claim \
            categories are the never-visited areas). The baseline is a state of \
            completeness, not an event: pick 1-2 gap areas per sitting. Use the \
            baseline_intake prompt to script a sitting.",
        input_schema = rmcp::handler::server::common::schema_for_type::<EmptyParams>()
    )]
    async fn run_baseline_intake(
        &self,
        params: LenientParameters<EmptyParams>,
    ) -> Result<CallToolResult, McpError> {
        let EmptyParams {} = match params.into_tool_input("run_baseline_intake") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        match claim::coverage(&*self.db).await {
            Ok(coverage) => {
                let total_claims = coverage.iter().map(|c| c.claims).sum();
                let total_unknowns = coverage.iter().map(|c| c.unknowns).sum();
                tool_json_result(&IntakeState {
                    coverage,
                    total_claims,
                    total_unknowns,
                })
            }
            Err(err) => domain_error(err),
        }
    }

    #[tool(
        name = "record_intake_answers",
        description = "Persist a batch of intake claims — what Steve CLAIMS, with honest \
            confidence (verified/recalled/unknown/not-done), never laundered into facts. \
            READ THE CLAIMS BACK to Steve before calling: an off-hand exaggeration must \
            not become canon. Include source_quote (his verbatim words) on every claim \
            you can. unknown = a task to resolve, never a nag.",
        input_schema = rmcp::handler::server::common::schema_for_type::<RecordIntakeAnswersInput>()
    )]
    async fn record_intake_answers(
        &self,
        params: LenientParameters<RecordIntakeAnswersInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("record_intake_answers") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        let claims = input
            .claims
            .into_iter()
            .map(ClaimInput::into_domain)
            .collect();
        domain_result(claim::record_batch(&*self.db, claims).await)
    }

    #[tool(
        name = "update_claim",
        description = "Revise a claim: fix a miscalibrated statement, upgrade confidence \
            after records are checked (unknown → verified), or downgrade an overstated \
            one. source_quote is immutable evidence and cannot be changed.",
        input_schema = rmcp::handler::server::common::schema_for_type::<UpdateClaimInput>()
    )]
    async fn update_claim(
        &self,
        params: LenientParameters<UpdateClaimInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("update_claim") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        let (id, update) = input.into_domain();
        domain_result(claim::update(&*self.db, id, update).await)
    }

    #[tool(
        name = "get_claims",
        description = "Read the claims registry, newest first, optionally filtered by \
            category, confidence, or subject ('self' = about Steve; 'father' etc. = \
            family history). Consult before reasoning about history, risk, or screenings.",
        input_schema = rmcp::handler::server::common::schema_for_type::<GetClaimsInput>()
    )]
    async fn get_claims(
        &self,
        params: LenientParameters<GetClaimsInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = match params.into_tool_input("get_claims") {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };
        domain_result(claim::list(&*self.db, input.into_domain()).await)
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for HealthieMcp {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::default();
        capabilities.tools = Some(ToolsCapability::default());
        capabilities.resources = Some(ResourcesCapability::default());
        capabilities.prompts = Some(PromptsCapability::default());
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new(
                "healthie-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "healthie MCP: the system of record for Steve's health — concerns, goals, \
                 protocols, observations, checkins, plans. The accountability loop is the \
                 product. Canonical flow: (1) ORIENT — get_briefing first, every conversation (or \
                 read healthie://briefing). (2) ACCOUNTABILITY — walk the previous plan's items \
                 and record_plan_outcome for each before planning anything new. (3) CHECKIN — \
                 start_checkin, then record_checkin_response per exchange AS IT HAPPENS \
                 (append-only; a dropped conversation resumes cleanly). (4) CAPTURE — \
                 log_observation / log_symptom anytime (origin 'self' when Steve reported it, \
                 'ai' for your inference); open_concern for new recurring problems; set_goal \
                 under concerns; start_protocol for deliberate interventions — but check \
                 get_protocol_history first: verdicts are permanent and nothing gets re-tried \
                 blind. record_protocol_outcome settles a protocol with a verdict + rationale. \
                 (5) PLAN — commit_plan with typed items (workout → calendar-bound, action → \
                 discrete to-do, guidance/nutrition → direction on the plan itself); healthie's \
                 copy is the source of truth, external pushes happen at the conversation layer. \
                 (6) CLOSE — complete_checkin with a summary the next checkin's accountability \
                 pass will read aloud. BASELINE — the claims registry holds health history as \
                 claims with honest confidence (verified/recalled/unknown/not-done), with \
                 source_quote provenance: run_baseline_intake shows coverage gaps; \
                 record_intake_answers captures claims (read them back first); update_claim \
                 revises/resolves; get_claims reads the registry — consult it before reasoning \
                 about history, risk, or screenings. The baseline_intake prompt scripts one \
                 sitting. The `checkin` prompt scripts this whole flow. Cadence- agnostic: a \
                 checkin covers 'since the last checkin'. update_profile for standing facts. All \
                 dates YYYY-MM-DD; timestamps RFC 3339 UTC.",
            )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let briefing = Resource::new(BRIEFING_URI, "Current briefing")
            .with_description(
                "The current health briefing: profile, last checkin, previous plan with outcomes, \
                 active concerns/goals/protocols, observations pending review.",
            )
            .with_mime_type("application/json");
        Ok(ListResourcesResult::with_all_items(vec![briefing]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match request.uri.as_str() {
            BRIEFING_URI => {
                let briefing = briefing::assemble(&*self.db, clock::today())
                    .await
                    .map_err(resource_error)?;
                let json = serde_json::to_string_pretty(&briefing).map_err(|err| {
                    tracing::error!(?err, "MCP resource: serialization failed");
                    McpError::internal_error("serialization error", None)
                })?;
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(json, BRIEFING_URI).with_mime_type("application/json"),
                ]))
            }
            other => Err(McpError::invalid_params(
                format!("unknown resource URI '{other}'; known URIs: {BRIEFING_URI}"),
                None,
            )),
        }
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

/// Resource-side counterpart of [`domain_error`] — resources have no tool-level
/// error channel, so recoverable variants map to typed protocol errors.
fn resource_error(err: DomainError) -> McpError {
    match err {
        DomainError::NotFound(msg) => McpError::resource_not_found(msg, None),
        DomainError::Invalid { .. } | DomainError::BadRequest(_) => {
            McpError::invalid_params(err.to_string(), None)
        }
        DomainError::Db(db_err) => {
            tracing::error!(?db_err, "MCP resource: database error");
            McpError::internal_error("database error", None)
        }
        DomainError::Internal(detail) => {
            tracing::error!(detail, "MCP resource: internal error");
            McpError::internal_error("internal error", None)
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
