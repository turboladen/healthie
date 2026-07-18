use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Lifecycle state of a goal. `GoalStatus::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum GoalStatus {
    #[sea_orm(string_value = "active")]
    #[serde(rename = "active")]
    Active,
    #[sea_orm(string_value = "achieved")]
    #[serde(rename = "achieved")]
    Achieved,
    #[sea_orm(string_value = "abandoned")]
    #[serde(rename = "abandoned")]
    Abandoned,
    #[sea_orm(string_value = "paused")]
    #[serde(rename = "paused")]
    Paused,
}

/// How a goal's `target_value` should be compared against the metric.
/// `GoalComparison::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum GoalComparison {
    #[sea_orm(string_value = "at-most")]
    #[serde(rename = "at-most")]
    AtMost,
    #[sea_orm(string_value = "at-least")]
    #[serde(rename = "at-least")]
    AtLeast,
    #[sea_orm(string_value = "range")]
    #[serde(rename = "range")]
    Range,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "goals")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub concern_id: Option<i32>,
    pub title: String,
    pub description: Option<String>,
    pub metric_kind: Option<String>,
    pub comparison: Option<GoalComparison>,
    pub target_value: Option<f64>,
    pub target_high: Option<f64>,
    pub target_date: Option<Date>,
    pub status: GoalStatus,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
