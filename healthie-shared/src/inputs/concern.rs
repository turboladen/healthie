use chrono::NaiveDate;

use crate::entities::concern_tag::ConcernTag;

#[derive(Debug)]
pub struct NewConcern {
    pub name: String,
    pub narrative: Option<String>,
    pub tags: Vec<ConcernTag>,
    /// Defaults to today.
    pub opened_on: Option<NaiveDate>,
}
