#[derive(Debug)]
pub struct NewConcern {
    pub name: String,
    pub narrative: Option<String>,
    pub tags: Vec<String>,
    /// YYYY-MM-DD; defaults to today.
    pub opened_on: Option<String>,
}
