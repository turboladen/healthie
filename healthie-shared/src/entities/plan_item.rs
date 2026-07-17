use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "plan_items")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub plan_id: i32,
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
    pub scheduled_for: Option<Date>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
