use chrono::NaiveDate;

#[derive(Debug)]
pub struct NewConcern {
    pub name: String,
    pub narrative: Option<String>,
    pub tags: Vec<String>,
    /// Defaults to today.
    pub opened_on: Option<NaiveDate>,
}
