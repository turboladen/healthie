use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "observations")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub occurred_at: DateTimeUtc,
    pub origin: String,
    pub kind: String,
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
