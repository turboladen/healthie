use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut t = Table::create();
        t.table(Alias::new("claims")).if_not_exists();
        // Not a singleton: autoincrement pk like `concerns`.
        t.col(
            ColumnDef::new(Alias::new("id"))
                .integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(ColumnDef::new(Alias::new("category")).text().not_null())
        .col(ColumnDef::new(Alias::new("statement")).text().not_null())
        .col(ColumnDef::new(Alias::new("confidence")).text().not_null())
        .col(ColumnDef::new(Alias::new("subject")).text())
        .col(ColumnDef::new(Alias::new("topic")).text())
        .col(ColumnDef::new(Alias::new("occurred_on")).date())
        .col(ColumnDef::new(Alias::new("source_quote")).text())
        .col(ColumnDef::new(Alias::new("concern_id")).integer())
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
        )
        .foreign_key(
            ForeignKey::create()
                .from(Alias::new("claims"), Alias::new("concern_id"))
                .to(Alias::new("concerns"), Alias::new("id"))
                .on_delete(ForeignKeyAction::SetNull),
        );
        manager.create_table(t).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("claims"))
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
        entities::claim::{self, ClaimCategory, ClaimConfidence},
        test_support::{date, datetime, test_db},
    };

    #[tokio::test]
    async fn claims_table_round_trips_with_enums_and_nullables() {
        let db = test_db().await;
        let now = datetime("2026-07-18 08:00:00");
        let saved = claim::ActiveModel {
            category: Set(ClaimCategory::FamilyHistory),
            statement: Set("Father: afib onset in his 60s".to_owned()),
            confidence: Set(ClaimConfidence::Recalled),
            subject: Set(Some("father".to_owned())),
            topic: Set(Some("afib".to_owned())),
            occurred_on: Set(Some(date("2026-07-18"))),
            source_quote: Set(Some("I think my dad's afib started in his 60s?".to_owned())),
            concern_id: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert claim");

        let found = claim::Entity::find_by_id(saved.id)
            .one(&db)
            .await
            .expect("query")
            .expect("row");
        assert_eq!(found.category, ClaimCategory::FamilyHistory);
        assert_eq!(found.confidence, ClaimConfidence::Recalled);
        assert_eq!(found.subject.as_deref(), Some("father"));
        // occurred_on exercises the nullable Date column round-trip.
        assert_eq!(found.occurred_on, Some(date("2026-07-18")));
    }

    #[tokio::test]
    async fn claims_concern_fk_enforced() {
        let db = test_db().await;
        let now = datetime("2026-07-18 08:00:00");
        let result = claim::ActiveModel {
            category: Set(ClaimCategory::Condition),
            statement: Set("x".to_owned()),
            confidence: Set(ClaimConfidence::Recalled),
            concern_id: Set(Some(9999)),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await;
        assert!(result.is_err(), "FK to missing concern must be rejected");
    }
}
