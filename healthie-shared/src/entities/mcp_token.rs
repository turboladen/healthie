//! Singleton MCP bearer token: argon2id PHC hash at rest plus an 8-char
//! cleartext fingerprint for identification. One operator, one row (id = 1).
//! The plaintext token is shown once at provision time and never stored.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mcp_token")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i32,
    /// Argon2id PHC string (`$argon2id$...`). Never serialize this outward in
    /// any future API — if this `Model` ever crosses a boundary, add
    /// `#[serde(skip_serializing)]` first.
    pub token_hash: String,
    /// First 8 chars of the plaintext (48 bits revealed, ~208 bits residual).
    pub fingerprint: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
