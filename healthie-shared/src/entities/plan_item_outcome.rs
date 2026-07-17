use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "plan_item_outcomes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub plan_item_id: i32,
    pub status: String,
    pub note: Option<String>,
    pub recorded_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
