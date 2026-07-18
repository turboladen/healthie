use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Lifecycle state of a concern.
///
/// Domain enums like this one replace the old `VALID_*` string whitelists: the
/// variants ARE the allowed values, so invalid input is unrepresentable rather
/// than validated at runtime. When M1b needs to enumerate the legal values
/// (e.g. to build an MCP tool schema), use the derived `EnumIter`:
/// `ConcernStatus::iter()` yields every variant, replacing the deleted
/// `VALID_STATUSES` const. Every domain enum in these entities follows the same
/// pattern.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ConcernStatus {
    #[sea_orm(string_value = "active")]
    #[serde(rename = "active")]
    Active,
    #[sea_orm(string_value = "monitoring")]
    #[serde(rename = "monitoring")]
    Monitoring,
    #[sea_orm(string_value = "resolved")]
    #[serde(rename = "resolved")]
    Resolved,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "concerns")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub status: ConcernStatus,
    pub narrative: Option<String>,
    pub opened_on: Date,
    pub resolved_on: Option<Date>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(all(test, feature = "schemars"))]
mod schemars_tests {
    use super::*;

    /// The advertised schema must carry the serde wire values (kebab-case),
    /// not the Rust variant names — the MCP surface depends on this.
    #[test]
    fn concern_status_schema_uses_wire_values() {
        let schema = schemars::schema_for!(ConcernStatus);
        let json = serde_json::to_string(&schema).expect("serialize schema");
        assert!(json.contains("\"active\""));
        assert!(json.contains("\"monitoring\""));
        assert!(json.contains("\"resolved\""));
        assert!(
            !json.contains("Resolved"),
            "Rust variant name leaked into schema"
        );
    }
}
