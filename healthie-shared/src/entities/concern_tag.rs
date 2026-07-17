use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Coarse body-system tag on a concern. `ConcernTag::iter()` enumerates the
/// legal values (replacing the deleted `VALID_TAGS` const).
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ConcernTag {
    #[sea_orm(string_value = "musculoskeletal")]
    #[serde(rename = "musculoskeletal")]
    Musculoskeletal,
    #[sea_orm(string_value = "neurological")]
    #[serde(rename = "neurological")]
    Neurological,
    #[sea_orm(string_value = "mental-health")]
    #[serde(rename = "mental-health")]
    MentalHealth,
    #[sea_orm(string_value = "cardiovascular")]
    #[serde(rename = "cardiovascular")]
    Cardiovascular,
    #[sea_orm(string_value = "metabolic")]
    #[serde(rename = "metabolic")]
    Metabolic,
    #[sea_orm(string_value = "nutrition")]
    #[serde(rename = "nutrition")]
    Nutrition,
    #[sea_orm(string_value = "preventive")]
    #[serde(rename = "preventive")]
    Preventive,
    #[sea_orm(string_value = "immune")]
    #[serde(rename = "immune")]
    Immune,
    #[sea_orm(string_value = "sleep")]
    #[serde(rename = "sleep")]
    Sleep,
    #[sea_orm(string_value = "general")]
    #[serde(rename = "general")]
    General,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "concern_tags")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub concern_id: i32,
    pub tag: ConcernTag,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
