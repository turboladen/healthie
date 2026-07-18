use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// How a plan item turned out. `OutcomeStatus::iter()` enumerates the legal
/// values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum OutcomeStatus {
    #[sea_orm(string_value = "done")]
    #[serde(rename = "done")]
    Done,
    #[sea_orm(string_value = "skipped")]
    #[serde(rename = "skipped")]
    Skipped,
    #[sea_orm(string_value = "partial")]
    #[serde(rename = "partial")]
    Partial,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "plan_item_outcomes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub plan_item_id: i32,
    pub status: OutcomeStatus,
    pub note: Option<String>,
    pub recorded_at: DateTimeUtc,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
