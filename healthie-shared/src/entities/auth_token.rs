//! Kinded bearer tokens (argon2id PHC hash at rest + 8-char cleartext
//! fingerprint). One row per `TokenKind` (`UNIQUE(kind)`). The plaintext is
//! shown once at provision time and never stored. Generalized from M1b's
//! singleton `mcp_token` (ADR-0005): the MCP operator token and the HAE
//! ingest token share this machinery but are distinct rows, so a leaked
//! ingest token can never drive MCP tools.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "auth_token")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub kind: TokenKind,
    /// Argon2id PHC string (`$argon2id$...`). Sensitive auth material:
    /// excluded from serialization so no future API can leak it by accident
    /// (`SeaORM` maps rows via `FromQueryResult`, not serde, so this only
    /// affects outward serialization).
    #[serde(skip_serializing)]
    pub token_hash: String,
    /// First 8 chars of the plaintext (48 bits revealed, ~208 bits residual).
    pub fingerprint: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

/// Which bearer token a row holds. Distinct rows → distinct blast radius.
#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum TokenKind {
    #[sea_orm(string_value = "mcp")]
    #[serde(rename = "mcp")]
    Mcp,
    #[sea_orm(string_value = "ingest")]
    #[serde(rename = "ingest")]
    Ingest,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
