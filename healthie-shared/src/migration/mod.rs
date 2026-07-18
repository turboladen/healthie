use sea_orm_migration::prelude::*;

mod m20260716_000001_initial_schema;
mod m20260717_000002_mcp_token;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260716_000001_initial_schema::Migration),
            Box::new(m20260717_000002_mcp_token::Migration),
        ]
    }
}
