use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // daily_metric: one curated row per (kind, date), autoincrement pk.
        let mut metric = Table::create();
        metric.table(Alias::new("daily_metric")).if_not_exists();
        metric
            .col(
                ColumnDef::new(Alias::new("id"))
                    .integer()
                    .not_null()
                    .auto_increment()
                    .primary_key(),
            )
            .col(ColumnDef::new(Alias::new("kind")).text().not_null())
            .col(ColumnDef::new(Alias::new("date")).date().not_null())
            .col(ColumnDef::new(Alias::new("value")).double().not_null())
            .col(ColumnDef::new(Alias::new("min")).double())
            .col(ColumnDef::new(Alias::new("max")).double())
            .col(ColumnDef::new(Alias::new("source")).text())
            // No SQL default on timestamps: services always Set both with the
            // sqlx DateTime<Utc> encoder (matches the initial schema convention).
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
        manager.create_table(metric).await?;

        // UNIQUE(kind, date): idempotent upsert key (the entity has no
        // #[sea_orm(unique)] marker; the index is the real enforcement).
        manager
            .create_index(
                Index::create()
                    .name("idx_daily_metric_kind_date")
                    .table(Alias::new("daily_metric"))
                    .col(Alias::new("kind"))
                    .col(Alias::new("date"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        // quarantined_metric: verbatim unknown points, write-once per
        // (raw_name, date). raw_point is a Json column (stored as text on SQLite).
        let mut quarantine = Table::create();
        quarantine
            .table(Alias::new("quarantined_metric"))
            .if_not_exists();
        quarantine
            .col(
                ColumnDef::new(Alias::new("id"))
                    .integer()
                    .not_null()
                    .auto_increment()
                    .primary_key(),
            )
            .col(ColumnDef::new(Alias::new("raw_name")).text().not_null())
            .col(ColumnDef::new(Alias::new("date")).date().not_null())
            .col(ColumnDef::new(Alias::new("raw_point")).json().not_null())
            .col(
                ColumnDef::new(Alias::new("created_at"))
                    .timestamp_with_time_zone()
                    .not_null(),
            );
        manager.create_table(quarantine).await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_quarantined_metric_name_date")
                    .table(Alias::new("quarantined_metric"))
                    .col(Alias::new("raw_name"))
                    .col(Alias::new("date"))
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Unique indexes drop with their tables on SQLite.
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("quarantined_metric"))
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("daily_metric"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};

    use crate::{
        entities::daily_metric::{self, MetricKind},
        test_support::{date, datetime, test_db},
    };

    #[tokio::test]
    async fn migration_creates_daily_metric_with_working_unique_index() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        daily_metric::ActiveModel {
            kind: Set(MetricKind::Steps),
            date: Set(date("2026-07-28")),
            value: Set(8123.0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert");

        let dup = daily_metric::ActiveModel {
            kind: Set(MetricKind::Steps),
            date: Set(date("2026-07-28")),
            value: Set(9000.0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await;
        assert!(dup.is_err(), "idx_daily_metric_kind_date must reject dups");
    }
}
