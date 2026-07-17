use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "goals")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub concern_id: Option<i32>,
    pub title: String,
    pub description: Option<String>,
    pub metric_kind: Option<String>,
    pub comparison: Option<String>,
    pub target_value: Option<f64>,
    pub target_high: Option<f64>,
    pub target_date: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
