use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Whether a plan item is a workout or a general action.
/// `PlanItemKind::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum PlanItemKind {
    #[sea_orm(string_value = "workout")]
    #[serde(rename = "workout")]
    Workout,
    #[sea_orm(string_value = "action")]
    #[serde(rename = "action")]
    Action,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "plan_items")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub plan_id: i32,
    pub kind: PlanItemKind,
    pub title: String,
    pub detail: Option<String>,
    pub scheduled_for: Option<Date>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
