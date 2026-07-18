//! LLM-facing tool input schemas. Deliberately separate structs from
//! `healthie_shared::inputs` — the MCP shape (doc-commented for the model,
//! schemars-derived) is decoupled from the persistence inputs; each maps over
//! via `into_domain()`. Vocabulary enums come straight from the domain
//! (`schemars` feature on healthie-shared) so schemas can never drift.

use chrono::{DateTime, NaiveDate, Utc};
use healthie_shared::{
    entities::{
        concern::ConcernStatus,
        concern_tag::ConcernTag,
        goal::GoalComparison,
        observation::{ObservationKind, ObservationOrigin},
        profile::Sex,
        protocol::{ProtocolKind, ProtocolVerdict},
    },
    inputs::{
        concern::NewConcern,
        goal::NewGoal,
        observation::NewObservation,
        profile::UpdateProfile,
        protocol::{NewProtocol, ProtocolOutcome},
    },
};
use schemars::JsonSchema;
use serde::Deserialize;

/// No-argument tools still advertise an (empty) object schema.
#[derive(Deserialize, JsonSchema)]
pub struct EmptyParams {}

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
