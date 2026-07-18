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

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

    use crate::{
        entities::mcp_token,
        test_support::{datetime, test_db},
    };

    #[tokio::test]
    async fn mcp_token_table_round_trips() {
        let db = test_db().await;
        let now = datetime("2026-07-17 08:00:00");
        // Never .save() with a Set PK — insert explicitly (repo convention).
        mcp_token::ActiveModel {
            id: Set(1),
            token_hash: Set("$argon2id$stub".to_owned()),
            fingerprint: Set("abcd1234".to_owned()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&db)
        .await
        .expect("insert singleton token row");

        let found = mcp_token::Entity::find_by_id(1)
            .one(&db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(found.fingerprint, "abcd1234");
    }
}
