use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Category of intervention. `ProtocolKind::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ProtocolKind {
    #[sea_orm(string_value = "diet")]
    #[serde(rename = "diet")]
    Diet,
    #[sea_orm(string_value = "exercise")]
    #[serde(rename = "exercise")]
    Exercise,
    #[sea_orm(string_value = "supplement")]
    #[serde(rename = "supplement")]
    Supplement,
    #[sea_orm(string_value = "therapy")]
    #[serde(rename = "therapy")]
    Therapy,
    #[sea_orm(string_value = "screening")]
    #[serde(rename = "screening")]
    Screening,
    #[sea_orm(string_value = "medication")]
    #[serde(rename = "medication")]
    Medication,
    #[sea_orm(string_value = "immunization")]
    #[serde(rename = "immunization")]
    Immunization,
    #[sea_orm(string_value = "monitoring")]
    #[serde(rename = "monitoring")]
    Monitoring,
    #[sea_orm(string_value = "habit")]
    #[serde(rename = "habit")]
    Habit,
}

/// Retrospective judgement on whether a protocol worked.
/// `ProtocolVerdict::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ProtocolVerdict {
    #[sea_orm(string_value = "worked")]
    #[serde(rename = "worked")]
    Worked,
    #[sea_orm(string_value = "didnt-work")]
    #[serde(rename = "didnt-work")]
    DidntWork,
    #[sea_orm(string_value = "mixed")]
    #[serde(rename = "mixed")]
    Mixed,
    #[sea_orm(string_value = "inconclusive")]
    #[serde(rename = "inconclusive")]
    Inconclusive,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "protocols")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub concern_id: Option<i32>,
    pub goal_id: Option<i32>,
    pub name: String,
    pub kind: ProtocolKind,
    pub purpose: Option<String>,
    pub schedule: Option<String>,
    pub started_on: Date,
    pub ended_on: Option<Date>,
    pub review_by: Option<Date>,
    pub verdict: Option<ProtocolVerdict>,
    pub verdict_rationale: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
