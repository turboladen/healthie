use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Singleton token row (id always 1); PK is NOT auto-increment.
        let mut t = Table::create();
        t.table(Alias::new("mcp_token")).if_not_exists();
        t.col(
            ColumnDef::new(Alias::new("id"))
                .integer()
                .not_null()
                .primary_key(),
        )
        .col(ColumnDef::new(Alias::new("token_hash")).text().not_null())
        .col(ColumnDef::new(Alias::new("fingerprint")).text().not_null())
        // No SQL default on timestamps: services always Set both with the sqlx
        // DateTime<Utc> encoder (matches the initial schema convention).
        .col(
            ColumnDef::new(Alias::new("created_at"))
                .timestamp_with_time_zone()
                .not_null(),
        )
        .col(
            ColumnDef::new(Alias::new("updated_at"))
                .timestamp_with_time_zone()
                .not_null(),
        );
        manager.create_table(t).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("mcp_token"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
// Round-trip coverage for the live token table moved to the auth_token
// migration (ADR-0005): m0005 drops this transient `mcp_token`, so a test that
// inserts into it here would fail against the fully migrated schema.
