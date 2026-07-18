use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Biological sex, used for reference ranges. `Sex::iter()` enumerates the
/// legal values (replacing the deleted `VALID_SEXES` const).
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum Sex {
    #[sea_orm(string_value = "male")]
    #[serde(rename = "male")]
    Male,
    #[sea_orm(string_value = "female")]
    #[serde(rename = "female")]
    Female,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "profile")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub date_of_birth: Option<Date>,
    pub sex: Option<Sex>,
    pub height_cm: Option<i32>,
    pub notes: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
