use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Generalize the M1b singleton (ADR-0005): drop `mcp_token`, create the
        // kinded `auth_token` (one row per kind, enforced by a unique index).
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("mcp_token"))
                    .if_exists()
                    .to_owned(),
            )
            .await?;

        let mut t = Table::create();
        t.table(Alias::new("auth_token")).if_not_exists();
        // Not a singleton anymore: autoincrement pk like `claims`.
        t.col(
            ColumnDef::new(Alias::new("id"))
                .integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(ColumnDef::new(Alias::new("kind")).text().not_null())
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
        manager.create_table(t).await?;

        // UNIQUE(kind): the real enforcement (the entity's `#[sea_orm(unique)]`
        // is inert metadata under our hand-written migrations).
        manager
            .create_index(
                Index::create()
                    .name("idx_auth_token_kind")
                    .table(Alias::new("auth_token"))
                    .col(Alias::new("kind"))
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Reverse symmetrically: drop auth_token (its unique index drops with
        // the table on SQLite), then recreate the m0002 singleton `mcp_token`.
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("auth_token"))
                    .if_exists()
                    .to_owned(),
            )
            .await?;

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
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};

    use crate::{
        entities::auth_token::{self, TokenKind},
        test_support::{datetime, test_db},
    };

    #[tokio::test]
    async fn auth_token_round_trips_and_kind_is_unique() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        for kind in [TokenKind::Mcp, TokenKind::Ingest] {
            auth_token::ActiveModel {
                kind: Set(kind),
                token_hash: Set("$argon2id$stub".to_owned()),
                fingerprint: Set("abcd1234".to_owned()),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(&db)
            .await
            .expect("insert token row");
        }
        let mcp = auth_token::Entity::find()
            .filter(auth_token::Column::Kind.eq(TokenKind::Mcp))
            .one(&db)
            .await
            .expect("query")
            .expect("row");
        assert_eq!(mcp.kind, TokenKind::Mcp);

        // UNIQUE(kind): a second mcp row is rejected.
        let dup = auth_token::ActiveModel {
            kind: Set(TokenKind::Mcp),
            token_hash: Set("$argon2id$other".to_owned()),
            fingerprint: Set("ffff0000".to_owned()),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await;
        assert!(dup.is_err(), "duplicate kind must violate the unique index");
    }
}
