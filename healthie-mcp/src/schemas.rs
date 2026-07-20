//! LLM-facing tool input schemas. Deliberately separate structs from
//! `healthie_shared::inputs` — the MCP shape (doc-commented for the model,
//! schemars-derived) is decoupled from the persistence inputs; each maps over
//! via `into_domain()`. Vocabulary enums come straight from the domain
//! (`schemars` feature on healthie-shared) so schemas can never drift.

use chrono::{DateTime, NaiveDate, Utc};
use healthie_shared::{
    entities::{
        claim::{ClaimCategory, ClaimConfidence},
        concern::ConcernStatus,
        concern_tag::ConcernTag,
        goal::GoalComparison,
        observation::{ObservationKind, ObservationOrigin},
        plan_item::PlanItemKind,
        plan_item_outcome::OutcomeStatus,
        profile::Sex,
        protocol::{ProtocolKind, ProtocolVerdict},
    },
    inputs::{
        claim::{ClaimFilter, NewClaim, UpdateClaim, is_self_sentinel},
        concern::NewConcern,
        goal::NewGoal,
        observation::NewObservation,
        plan::{NewPlan, NewPlanItem},
        profile::UpdateProfile,
        protocol::{NewProtocol, ProtocolOutcome},
    },
};
use schemars::JsonSchema;
use serde::Deserialize;

/// No-argument tools still advertise an (empty) object schema.
#[derive(Deserialize, JsonSchema)]
pub struct EmptyParams {}

/// Arguments for the `checkin` prompt.
#[derive(Deserialize, JsonSchema)]
pub struct CheckinPromptArgs {
    /// Anything specific on your mind to start from (optional).
    pub focus: Option<String>,
}

/// Arguments for the `baseline_intake` prompt.
#[derive(Deserialize, JsonSchema)]
pub struct BaselineIntakePromptArgs {
    /// System area to focus this sitting on (optional) — e.g. "screenings",
    /// "cardiovascular + family history".
    pub area: Option<String>,
}

/// Open a new health concern — the top of the Concern → Goal → Protocol chain.
#[derive(Deserialize, JsonSchema)]
pub struct OpenConcernInput {
    /// Short name, e.g. "Right shoulder pain".
    pub name: String,
    /// Freeform narrative: history, context, what prompted opening it.
    pub narrative: Option<String>,
    /// Body-system tags for later filtering.
    #[serde(default)]
    pub tags: Vec<ConcernTag>,
    /// Date opened; defaults to today.
    pub opened_on: Option<NaiveDate>,
}

impl OpenConcernInput {
    #[must_use]
    pub fn into_domain(self) -> NewConcern {
        NewConcern {
            name: self.name,
            narrative: self.narrative,
            tags: self.tags,
            opened_on: self.opened_on,
        }
    }
}

/// Change a concern's status; `resolved` permanently stamps `resolved_on`.
#[derive(Deserialize, JsonSchema)]
pub struct UpdateConcernStatusInput {
    /// Concern id, from `get_briefing` or `open_concern`.
    pub concern_id: i32,
    pub status: ConcernStatus,
    /// Optional dated note appended to the concern's narrative.
    pub note: Option<String>,
}

/// Set a goal, optionally under a concern. Measurable goals give a
/// `comparison` + `target_value` (+ `target_high` for `range`).
#[derive(Deserialize, JsonSchema)]
pub struct SetGoalInput {
    /// Parent concern id, if this goal belongs to one.
    pub concern_id: Option<i32>,
    pub title: String,
    pub description: Option<String>,
    /// Metric identifier this goal tracks, e.g. `body_mass_lbs`.
    pub metric_kind: Option<String>,
    pub comparison: Option<GoalComparison>,
    pub target_value: Option<f64>,
    /// Upper bound; required when comparison is `range`.
    pub target_high: Option<f64>,
    pub target_date: Option<NaiveDate>,
}

impl SetGoalInput {
    #[must_use]
    pub fn into_domain(self) -> NewGoal {
        NewGoal {
            concern_id: self.concern_id,
            title: self.title,
            description: self.description,
            metric_kind: self.metric_kind,
            comparison: self.comparison,
            target_value: self.target_value,
            target_high: self.target_high,
            target_date: self.target_date,
        }
    }
}

/// Start a protocol: a deliberate intervention with a purpose and a review-by
/// date, whose outcome will be permanently recorded.
#[derive(Deserialize, JsonSchema)]
pub struct StartProtocolInput {
    pub concern_id: Option<i32>,
    pub goal_id: Option<i32>,
    /// e.g. "Creatine 5g daily".
    pub name: String,
    pub kind: ProtocolKind,
    /// Why this protocol exists — what it's meant to change.
    pub purpose: Option<String>,
    /// Freetext schedule, e.g. "daily with breakfast".
    pub schedule: Option<String>,
    /// Defaults to today.
    pub started_on: Option<NaiveDate>,
    /// When to evaluate whether it worked.
    pub review_by: Option<NaiveDate>,
}

impl StartProtocolInput {
    #[must_use]
    pub fn into_domain(self) -> NewProtocol {
        NewProtocol {
            concern_id: self.concern_id,
            goal_id: self.goal_id,
            name: self.name,
            kind: self.kind,
            purpose: self.purpose,
            schedule: self.schedule,
            started_on: self.started_on,
            review_by: self.review_by,
        }
    }
}

/// Record a protocol's permanent verdict — nothing gets re-suggested blind.
#[derive(Deserialize, JsonSchema)]
pub struct RecordProtocolOutcomeInput {
    pub protocol_id: i32,
    pub verdict: ProtocolVerdict,
    /// Required: why this verdict. This is what future planning reads.
    pub rationale: String,
    /// Defaults to today.
    pub ended_on: Option<NaiveDate>,
}

impl RecordProtocolOutcomeInput {
    #[must_use]
    pub fn into_domain(self) -> (i32, ProtocolOutcome) {
        (
            self.protocol_id,
            ProtocolOutcome {
                verdict: self.verdict,
                rationale: self.rationale,
                ended_on: self.ended_on,
            },
        )
    }
}

/// Update profile fields. Set-only: omitted fields are untouched; clearing a
/// stored value is not expressible on the MCP surface.
#[derive(Deserialize, JsonSchema)]
pub struct UpdateProfileInput {
    pub date_of_birth: Option<NaiveDate>,
    pub sex: Option<Sex>,
    pub height_cm: Option<i32>,
    /// Standing context worth every briefing carrying.
    pub notes: Option<String>,
}

impl UpdateProfileInput {
    #[must_use]
    pub fn into_domain(self) -> UpdateProfile {
        UpdateProfile {
            date_of_birth: self.date_of_birth.map(Some),
            sex: self.sex.map(Some),
            height_cm: self.height_cm.map(Some),
            notes: self.notes.map(Some),
        }
    }
}

/// Log a freeform observation (a note — for symptoms use `log_symptom`).
#[derive(Deserialize, JsonSchema)]
pub struct LogObservationInput {
    /// `self` when relaying something Steve reported (auto-marked reviewed);
    /// `ai` for your own inference (queued for his review).
    pub origin: ObservationOrigin,
    pub body: String,
    /// Link to a concern id when clearly related.
    pub concern_id: Option<i32>,
    /// When it happened (RFC 3339 UTC); defaults to now.
    pub occurred_at: Option<DateTime<Utc>>,
}

impl LogObservationInput {
    #[must_use]
    pub fn into_domain(self) -> NewObservation {
        NewObservation {
            origin: self.origin,
            kind: ObservationKind::Note,
            body: self.body,
            severity: None,
            concern_id: self.concern_id,
            occurred_at: self.occurred_at,
        }
    }
}

/// Log a symptom, optionally with 1–10 severity.
#[derive(Deserialize, JsonSchema)]
pub struct LogSymptomInput {
    /// `self` when relaying something Steve reported; `ai` for inference.
    pub origin: ObservationOrigin,
    pub body: String,
    /// 1 (barely noticeable) to 10 (worst imaginable).
    pub severity: Option<i32>,
    pub concern_id: Option<i32>,
    /// When it happened (RFC 3339 UTC); defaults to now.
    pub occurred_at: Option<DateTime<Utc>>,
}

impl LogSymptomInput {
    #[must_use]
    pub fn into_domain(self) -> NewObservation {
        NewObservation {
            origin: self.origin,
            kind: ObservationKind::Symptom,
            body: self.body,
            severity: self.severity,
            concern_id: self.concern_id,
            occurred_at: self.occurred_at,
        }
    }
}

/// One question/answer exchange inside an open checkin. Append-only — a
/// dropped conversation leaves a valid, resumable partial checkin.
#[derive(Deserialize, JsonSchema)]
pub struct RecordCheckinResponseInput {
    /// From `start_checkin`.
    pub checkin_id: i32,
    /// The question as asked.
    pub question: String,
    /// The answer as given (summarize faithfully, don't editorialize).
    pub answer: String,
    /// Concern this exchange was about, when clearly one.
    pub concern_id: Option<i32>,
}

/// Close a checkin with a summary the NEXT checkin's accountability pass will
/// read aloud.
#[derive(Deserialize, JsonSchema)]
pub struct CompleteCheckinInput {
    pub checkin_id: i32,
    pub summary: String,
}

/// A typed plan item. `workout` items are calendar-bound; `action` items are
/// discrete to-dos; guidance/nutrition live on the plan itself.
#[derive(Deserialize, JsonSchema)]
pub struct PlanItemInput {
    pub kind: PlanItemKind,
    pub title: String,
    pub detail: Option<String>,
    pub scheduled_for: Option<NaiveDate>,
}

/// Commit the plan agreed in conversation — healthie's copy is the source of
/// truth; pushing items to calendar/tasks happens at the conversation layer.
#[derive(Deserialize, JsonSchema)]
pub struct CommitPlanInput {
    /// Checkin this plan came out of.
    pub checkin_id: Option<i32>,
    /// Defaults to today.
    pub starts_on: Option<NaiveDate>,
    /// Defaults to 7.
    pub horizon_days: Option<i32>,
    /// Standing guidance for the horizon (not item-shaped).
    pub guidance: Option<String>,
    /// Nutrition direction for the horizon.
    pub nutrition: Option<String>,
    pub items: Vec<PlanItemInput>,
}

impl CommitPlanInput {
    #[must_use]
    pub fn into_domain(self) -> NewPlan {
        NewPlan {
            checkin_id: self.checkin_id,
            starts_on: self.starts_on,
            horizon_days: self.horizon_days,
            guidance: self.guidance,
            nutrition: self.nutrition,
            items: self
                .items
                .into_iter()
                .map(|item| NewPlanItem {
                    kind: item.kind,
                    title: item.title,
                    detail: item.detail,
                    scheduled_for: item.scheduled_for,
                })
                .collect(),
        }
    }
}

/// What actually happened to a plan item. Re-recording replaces the outcome.
#[derive(Deserialize, JsonSchema)]
pub struct RecordPlanOutcomeInput {
    /// Plan item id, from the briefing's `previous_plan`.
    pub plan_item_id: i32,
    pub status: OutcomeStatus,
    /// Context: why skipped, how partial, etc.
    pub note: Option<String>,
}

/// Canonical single-value subject mapping, shared by the create path and the
/// read filter (the sentinel predicate itself lives in healthie-shared, so
/// the MCP boundary and the domain guard can never disagree — ADR-0004 §2):
/// "self" (any case) and blank both mean about-Steve → None; anything else
/// is a relative name, trimmed.
fn canonical_subject(subject: &str) -> Option<String> {
    let trimmed = subject.trim();
    if trimmed.is_empty() || is_self_sentinel(trimmed) {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// One claim as captured during intake. Statement is the distilled record;
/// quote is the verbatim words it came from.
#[derive(Deserialize, JsonSchema)]
pub struct ClaimInput {
    pub category: ClaimCategory,
    /// The distilled claim, in plain words.
    pub statement: String,
    /// How sure Steve is: verified (records seen) / recalled (memory) /
    /// unknown (a task to resolve) / not-done (confirmed never happened).
    pub confidence: ClaimConfidence,
    /// Omit when the claim is about Steve ("self" or blank is treated as
    /// omitted); else the relative ("father").
    pub subject: Option<String>,
    /// Normalizable key for rules to query later, e.g. "colonoscopy".
    pub topic: Option<String>,
    /// When the claimed thing happened, if dateable (YYYY-MM-DD).
    pub occurred_on: Option<NaiveDate>,
    /// Verbatim (or near-verbatim) words that produced this claim —
    /// provenance that travels with it. Strongly encouraged.
    pub source_quote: Option<String>,
    /// Link to a concern id when clearly related.
    pub concern_id: Option<i32>,
}

impl ClaimInput {
    #[must_use]
    pub fn into_domain(self) -> NewClaim {
        NewClaim {
            category: self.category,
            statement: self.statement,
            confidence: self.confidence,
            subject: self.subject.and_then(|s| canonical_subject(&s)),
            topic: self.topic,
            occurred_on: self.occurred_on,
            source_quote: self.source_quote,
            concern_id: self.concern_id,
        }
    }
}

/// Record a batch of intake claims. Read them back to Steve BEFORE calling.
#[derive(Deserialize, JsonSchema)]
pub struct RecordIntakeAnswersInput {
    /// Required in the advertised schema on purpose: for an LLM caller the
    /// schema is the primary coaching surface, so `claims` stays in the
    /// `required` list (a missing key gets the schema-hint error instead).
    pub claims: Vec<ClaimInput>,
}

/// Revise a claim (fix calibration, resolve an unknown). `source_quote` is
/// immutable evidence and cannot be changed. Set-only: omitted fields are
/// untouched, and clearing a stored value is not expressible on the MCP
/// surface (matches `update_profile`) — with one exception: `subject`
/// accepts "self" to reclassify a claim as being about Steve.
#[derive(Deserialize, JsonSchema)]
pub struct UpdateClaimInput {
    /// From `get_claims` / `record_intake_answers` / the briefing.
    pub claim_id: i32,
    pub category: Option<ClaimCategory>,
    pub statement: Option<String>,
    pub confidence: Option<ClaimConfidence>,
    /// The relative this claim is about — or "self" to reclassify the claim
    /// as being about Steve (stores NULL).
    pub subject: Option<String>,
    pub topic: Option<String>,
    pub occurred_on: Option<NaiveDate>,
    pub concern_id: Option<i32>,
}

impl UpdateClaimInput {
    #[must_use]
    pub fn into_domain(self) -> (i32, UpdateClaim) {
        (
            self.claim_id,
            UpdateClaim {
                category: self.category,
                statement: self.statement,
                confidence: self.confidence,
                // "self" → Some(None): clears subject to NULL, reclassifying
                // the claim as about Steve — the ONE clear affordance here.
                // Blank is deliberately NOT a clear: it passes through so the
                // service rejects it loudly (ambiguous intent ≠ silent wipe).
                subject: self
                    .subject
                    .map(|s| if is_self_sentinel(&s) { None } else { Some(s) }),
                topic: self.topic.map(Some),
                occurred_on: self.occurred_on.map(Some),
                concern_id: self.concern_id.map(Some),
            },
        )
    }
}

/// Read the claims registry with optional filters.
#[derive(Deserialize, JsonSchema)]
pub struct GetClaimsInput {
    pub category: Option<ClaimCategory>,
    pub confidence: Option<ClaimConfidence>,
    /// "self" (any case) or blank → only claims about Steve; any other value
    /// → that relative (e.g. "father"); omit → all subjects.
    pub subject: Option<String>,
}

impl GetClaimsInput {
    #[must_use]
    pub fn into_domain(self) -> ClaimFilter {
        ClaimFilter {
            category: self.category,
            confidence: self.confidence,
            // Same canonicalizer as the create path — read and write can
            // never disagree on what a subject value means.
            subject: self.subject.map(|s| canonical_subject(&s)),
        }
    }
}
