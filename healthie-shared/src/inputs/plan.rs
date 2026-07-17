use chrono::NaiveDate;

#[derive(Debug)]
pub struct NewPlan {
    pub checkin_id: Option<i32>,
    /// Defaults to today.
    pub starts_on: Option<NaiveDate>,
    /// Defaults to 7.
    pub horizon_days: Option<i32>,
    pub guidance: Option<String>,
    pub nutrition: Option<String>,
    pub items: Vec<NewPlanItem>,
}

#[derive(Debug)]
pub struct NewPlanItem {
    /// "workout" or "action".
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
    /// For time-bound items Claude pushes to the calendar.
    pub scheduled_for: Option<NaiveDate>,
}
