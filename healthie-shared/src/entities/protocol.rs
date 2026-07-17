use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "protocols")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub concern_id: Option<i32>,
    pub goal_id: Option<i32>,
    pub name: String,
    pub kind: String,
    pub purpose: Option<String>,
    pub schedule: Option<String>,
    pub started_on: Date,
    pub ended_on: Option<Date>,
    pub review_by: Option<Date>,
    pub verdict: Option<String>,
    pub verdict_rationale: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
