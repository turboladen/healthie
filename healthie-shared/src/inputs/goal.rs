#[derive(Debug)]
pub struct NewGoal {
    pub concern_id: Option<i32>,
    pub title: String,
    pub description: Option<String>,
    /// e.g. `body_mass_lbs`, `resting_heart_rate` — free text until M2 metrics land.
    pub metric_kind: Option<String>,
    pub comparison: Option<String>,
    pub target_value: Option<f64>,
    pub target_high: Option<f64>,
    /// YYYY-MM-DD
    pub target_date: Option<String>,
}
