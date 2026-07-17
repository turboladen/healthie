#[derive(Debug)]
pub struct NewProtocol {
    pub concern_id: Option<i32>,
    pub goal_id: Option<i32>,
    pub name: String,
    pub kind: String,
    pub purpose: Option<String>,
    /// Freetext, e.g. "400mg with dinner, daily".
    pub schedule: Option<String>,
    /// YYYY-MM-DD; defaults to today.
    pub started_on: Option<String>,
    /// YYYY-MM-DD; when to re-evaluate whether this is still needed.
    pub review_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProtocolOutcome {
    pub verdict: String,
    /// Mandatory: WHY — this is the permanent record that prevents re-suggesting.
    pub rationale: String,
    /// YYYY-MM-DD; defaults to today.
    pub ended_on: Option<String>,
}
