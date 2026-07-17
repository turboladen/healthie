use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Who or what produced an observation. `ObservationOrigin::iter()` enumerates
/// the legal values.
///
/// The JSON wire value is `self` (via serde), but the DB `string_value` is
/// `self_reported`: `SeaORM` 1.1's `DeriveActiveEnum` Pascal-cases each
/// `string_value` into an internal marker-enum identifier, and `self` becomes
/// the reserved keyword `Self`, which will not compile. The DB token is an
/// internal detail (greenfield schema, no query filters on `origin`), so
/// diverging it from the wire value costs nothing while keeping the MCP wire
/// contract byte-identical.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ObservationOrigin {
    #[sea_orm(string_value = "self_reported")]
    #[serde(rename = "self")]
    SelfReported,
    #[sea_orm(string_value = "ai")]
    #[serde(rename = "ai")]
    Ai,
    #[sea_orm(string_value = "rules")]
    #[serde(rename = "rules")]
    Rules,
}

/// Whether an observation is a free note or a graded symptom.
/// `ObservationKind::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ObservationKind {
    #[sea_orm(string_value = "note")]
    #[serde(rename = "note")]
    Note,
    #[sea_orm(string_value = "symptom")]
    #[serde(rename = "symptom")]
    Symptom,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "observations")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub occurred_at: DateTimeUtc,
    pub origin: ObservationOrigin,
    pub kind: ObservationKind,
    pub body: String,
    pub severity: Option<i32>,
    pub concern_id: Option<i32>,
    pub reviewed: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
