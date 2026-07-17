use chrono::{DateTime, Utc};

use crate::entities::observation::{ObservationKind, ObservationOrigin};

#[derive(Debug)]
pub struct NewObservation {
    /// `self` (you felt it), `ai` (Claude spotted it in data), `rules` (deterministic flag).
    pub origin: ObservationOrigin,
    pub kind: ObservationKind,
    pub body: String,
    /// 1-10, symptoms only in practice.
    pub severity: Option<i32>,
    pub concern_id: Option<i32>,
    /// Defaults to now.
    pub occurred_at: Option<DateTime<Utc>>,
}
