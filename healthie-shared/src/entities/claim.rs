//! Claims-with-confidence registry (ADR-0004): what Steve *claims* about his
//! health history, never laundered into facts. Prose `statement` is the
//! primary record; `topic`/`occurred_on` give M3's screening rules something
//! queryable; `source_quote` is immutable provenance — the verbatim words the
//! claim was distilled from, so calibration drift stays visible; `subject`
//! absent means the claim is about Steve, else the relative ("father") — the
//! literal "self" is reserved (canonicalized to absent at the MCP boundary,
//! rejected as a stored value by the services; ADR-0004 §2).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "claims")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub category: ClaimCategory,
    pub statement: String,
    pub confidence: ClaimConfidence,
    pub subject: Option<String>,
    pub topic: Option<String>,
    pub occurred_on: Option<Date>,
    /// Immutable evidence — no update path touches this.
    pub source_quote: Option<String>,
    pub concern_id: Option<i32>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

/// What the claim is about. `ClaimCategory::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ClaimCategory {
    #[sea_orm(string_value = "family-history")]
    #[serde(rename = "family-history")]
    FamilyHistory,
    #[sea_orm(string_value = "condition")]
    #[serde(rename = "condition")]
    Condition,
    #[sea_orm(string_value = "surgery")]
    #[serde(rename = "surgery")]
    Surgery,
    #[sea_orm(string_value = "injury")]
    #[serde(rename = "injury")]
    Injury,
    #[sea_orm(string_value = "screening")]
    #[serde(rename = "screening")]
    Screening,
    #[sea_orm(string_value = "medication")]
    #[serde(rename = "medication")]
    Medication,
    #[sea_orm(string_value = "supplement")]
    #[serde(rename = "supplement")]
    Supplement,
    #[sea_orm(string_value = "allergy")]
    #[serde(rename = "allergy")]
    Allergy,
    #[sea_orm(string_value = "mental-health")]
    #[serde(rename = "mental-health")]
    MentalHealth,
    #[sea_orm(string_value = "lifestyle")]
    #[serde(rename = "lifestyle")]
    Lifestyle,
    /// Coached catch-all for anything the vocabulary didn't anticipate.
    /// (Out-of-vocabulary category VALUES are rejected at the schema
    /// boundary with a retryable error — never silently dropped or coerced;
    /// see ADR-0004 §5.)
    #[sea_orm(string_value = "general")]
    #[serde(rename = "general")]
    General,
}

/// ADR-0002-fixed vocabulary: claims carry how sure Steve is, never
/// laundered certainty. `Unknown` is a task to resolve, never a nag.
/// `ClaimConfidence::iter()` enumerates the legal values.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum ClaimConfidence {
    #[sea_orm(string_value = "verified")]
    #[serde(rename = "verified")]
    Verified,
    #[sea_orm(string_value = "recalled")]
    #[serde(rename = "recalled")]
    Recalled,
    #[sea_orm(string_value = "unknown")]
    #[serde(rename = "unknown")]
    Unknown,
    #[sea_orm(string_value = "not-done")]
    #[serde(rename = "not-done")]
    NotDone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(all(test, feature = "schemars"))]
mod schemars_tests {
    use super::*;

    /// The advertised schema must carry the serde wire values (kebab-case),
    /// not the Rust variant names — the MCP intake surface depends on this,
    /// and these enums are hand-authored with per-variant renames.
    #[test]
    fn claim_category_schema_uses_wire_values() {
        let schema = schemars::schema_for!(ClaimCategory);
        let json = serde_json::to_string(&schema).expect("serialize schema");
        assert!(json.contains("\"family-history\""));
        assert!(json.contains("\"mental-health\""));
        assert!(json.contains("\"general\""));
        assert!(
            !json.contains("FamilyHistory"),
            "Rust variant name leaked into schema"
        );
    }

    #[test]
    fn claim_confidence_schema_uses_wire_values() {
        let schema = schemars::schema_for!(ClaimConfidence);
        let json = serde_json::to_string(&schema).expect("serialize schema");
        assert!(json.contains("\"verified\""));
        assert!(json.contains("\"not-done\""));
        assert!(
            !json.contains("NotDone"),
            "Rust variant name leaked into schema"
        );
    }
}
