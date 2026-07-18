use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Coarse body-system tag on a concern. `ConcernTag::iter()` enumerates the
/// legal values (replacing the deleted `VALID_TAGS` const).
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
    #[sea_orm(string_value = "respiratory")]
    #[serde(rename = "respiratory")]
    Respiratory,
    #[sea_orm(string_value = "digestive")]
    #[serde(rename = "digestive")]
    Digestive,
    #[sea_orm(string_value = "endocrine")]
    #[serde(rename = "endocrine")]
    Endocrine,
    #[sea_orm(string_value = "dermatologic")]
    #[serde(rename = "dermatologic")]
    Dermatologic,
    #[sea_orm(string_value = "dental")]
    #[serde(rename = "dental")]
    Dental,
    #[sea_orm(string_value = "sensory")]
    #[serde(rename = "sensory")]
    Sensory,
    #[sea_orm(string_value = "fitness")]
    #[serde(rename = "fitness")]
    Fitness,
    #[sea_orm(string_value = "spiritual")]
    #[serde(rename = "spiritual")]
    Spiritual,
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
