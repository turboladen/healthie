use chrono::NaiveDate;

use crate::entities::protocol::{ProtocolKind, ProtocolVerdict};

#[derive(Debug)]
pub struct NewProtocol {
    pub concern_id: Option<i32>,
    pub goal_id: Option<i32>,
    pub name: String,
    pub kind: ProtocolKind,
    pub purpose: Option<String>,
    /// Freetext, e.g. "400mg with dinner, daily".
    pub schedule: Option<String>,
    /// Defaults to today.
    pub started_on: Option<NaiveDate>,
    /// When to re-evaluate whether this is still needed.
    pub review_by: Option<NaiveDate>,
}

#[derive(Debug, Clone)]
pub struct ProtocolOutcome {
    pub verdict: ProtocolVerdict,
    /// Mandatory: WHY — this is the permanent record that prevents re-suggesting.
    pub rationale: String,
    /// Defaults to today.
    pub ended_on: Option<NaiveDate>,
}
