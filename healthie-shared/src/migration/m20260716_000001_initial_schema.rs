use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

fn timestamps(t: &mut TableCreateStatement) -> &mut TableCreateStatement {
    // No SQL default: services always Set both timestamps with the sqlx
    // DateTime<Utc> encoder. A `datetime('now')` default would write
    // space-formatted text that sorts before every RFC3339 row we write, so
    // dropping it both removes a format-corruption hazard and makes a forgotten
    // Set fail fast (NOT NULL violation) instead of silently misordering.
    t.col(
        ColumnDef::new(Alias::new("created_at"))
            .timestamp_with_time_zone()
            .not_null(),
    )
    .col(
        ColumnDef::new(Alias::new("updated_at"))
            .timestamp_with_time_zone()
            .not_null(),
    )
}

fn pk(t: &mut TableCreateStatement, id: Alias) -> &mut TableCreateStatement {
    t.col(
        ColumnDef::new(id)
            .integer()
            .not_null()
            .auto_increment()
            .primary_key(),
    )
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    #[allow(clippy::too_many_lines)]
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // profile — singleton row (id always 1)
        let mut t = Table::create();
        t.table(Alias::new("profile")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("date_of_birth")).date())
            .col(ColumnDef::new(Alias::new("sex")).text())
            .col(ColumnDef::new(Alias::new("height_cm")).integer())
            .col(ColumnDef::new(Alias::new("notes")).text());
        timestamps(&mut t);
        manager.create_table(t).await?;

        // concerns
        let mut t = Table::create();
        t.table(Alias::new("concerns")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("name")).text().not_null())
            .col(
                ColumnDef::new(Alias::new("status"))
                    .text()
                    .not_null()
                    .default("active"),
            )
            .col(ColumnDef::new(Alias::new("narrative")).text())
            .col(ColumnDef::new(Alias::new("opened_on")).date().not_null())
            .col(ColumnDef::new(Alias::new("resolved_on")).date());
        timestamps(&mut t);
        manager.create_table(t).await?;

        // concern_tags
        let mut t = Table::create();
        t.table(Alias::new("concern_tags")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(
            ColumnDef::new(Alias::new("concern_id"))
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(Alias::new("tag")).text().not_null())
        .foreign_key(
            ForeignKey::create()
                .from(Alias::new("concern_tags"), Alias::new("concern_id"))
                .to(Alias::new("concerns"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        );
        timestamps(&mut t);
        manager.create_table(t).await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_concern_tags_unique")
                    .table(Alias::new("concern_tags"))
                    .col(Alias::new("concern_id"))
                    .col(Alias::new("tag"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        // goals
        let mut t = Table::create();
        t.table(Alias::new("goals")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("concern_id")).integer())
            .col(ColumnDef::new(Alias::new("title")).text().not_null())
            .col(ColumnDef::new(Alias::new("description")).text())
            .col(ColumnDef::new(Alias::new("metric_kind")).text())
            .col(ColumnDef::new(Alias::new("comparison")).text())
            .col(ColumnDef::new(Alias::new("target_value")).double())
            .col(ColumnDef::new(Alias::new("target_high")).double())
            .col(ColumnDef::new(Alias::new("target_date")).date())
            .col(
                ColumnDef::new(Alias::new("status"))
                    .text()
                    .not_null()
                    .default("active"),
            )
            .foreign_key(
                ForeignKey::create()
                    .from(Alias::new("goals"), Alias::new("concern_id"))
                    .to(Alias::new("concerns"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::SetNull),
            );
        timestamps(&mut t);
        manager.create_table(t).await?;

        // protocols
        let mut t = Table::create();
        t.table(Alias::new("protocols")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("concern_id")).integer())
            .col(ColumnDef::new(Alias::new("goal_id")).integer())
            .col(ColumnDef::new(Alias::new("name")).text().not_null())
            .col(ColumnDef::new(Alias::new("kind")).text().not_null())
            .col(ColumnDef::new(Alias::new("purpose")).text())
            .col(ColumnDef::new(Alias::new("schedule")).text())
            .col(ColumnDef::new(Alias::new("started_on")).date().not_null())
            .col(ColumnDef::new(Alias::new("ended_on")).date())
            .col(ColumnDef::new(Alias::new("review_by")).date())
            .col(ColumnDef::new(Alias::new("verdict")).text())
            .col(ColumnDef::new(Alias::new("verdict_rationale")).text())
            .foreign_key(
                ForeignKey::create()
                    .from(Alias::new("protocols"), Alias::new("concern_id"))
                    .to(Alias::new("concerns"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::SetNull),
            )
            .foreign_key(
                ForeignKey::create()
                    .from(Alias::new("protocols"), Alias::new("goal_id"))
                    .to(Alias::new("goals"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::SetNull),
            );
        timestamps(&mut t);
        manager.create_table(t).await?;

        // observations
        let mut t = Table::create();
        t.table(Alias::new("observations")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(
            ColumnDef::new(Alias::new("occurred_at"))
                .timestamp_with_time_zone()
                .not_null(),
        )
        .col(ColumnDef::new(Alias::new("origin")).text().not_null())
        .col(
            ColumnDef::new(Alias::new("kind"))
                .text()
                .not_null()
                .default("note"),
        )
        .col(ColumnDef::new(Alias::new("body")).text().not_null())
        .col(ColumnDef::new(Alias::new("severity")).integer())
        .col(ColumnDef::new(Alias::new("concern_id")).integer())
        .col(
            ColumnDef::new(Alias::new("reviewed"))
                .integer()
                .not_null()
                .default(0),
        )
        .foreign_key(
            ForeignKey::create()
                .from(Alias::new("observations"), Alias::new("concern_id"))
                .to(Alias::new("concerns"), Alias::new("id"))
                .on_delete(ForeignKeyAction::SetNull),
        );
        timestamps(&mut t);
        manager.create_table(t).await?;

        // checkins
        let mut t = Table::create();
        t.table(Alias::new("checkins")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(
            ColumnDef::new(Alias::new("started_at"))
                .timestamp_with_time_zone()
                .not_null(),
        )
        .col(ColumnDef::new(Alias::new("completed_at")).timestamp_with_time_zone())
        .col(ColumnDef::new(Alias::new("summary")).text());
        timestamps(&mut t);
        manager.create_table(t).await?;

        // checkin_responses
        let mut t = Table::create();
        t.table(Alias::new("checkin_responses")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(
            ColumnDef::new(Alias::new("checkin_id"))
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(Alias::new("question")).text().not_null())
        .col(ColumnDef::new(Alias::new("answer")).text().not_null())
        .col(ColumnDef::new(Alias::new("concern_id")).integer())
        .foreign_key(
            ForeignKey::create()
                .from(Alias::new("checkin_responses"), Alias::new("checkin_id"))
                .to(Alias::new("checkins"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(Alias::new("checkin_responses"), Alias::new("concern_id"))
                .to(Alias::new("concerns"), Alias::new("id"))
                .on_delete(ForeignKeyAction::SetNull),
        );
        timestamps(&mut t);
        manager.create_table(t).await?;

        // plans
        let mut t = Table::create();
        t.table(Alias::new("plans")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("checkin_id")).integer())
            .col(ColumnDef::new(Alias::new("starts_on")).date().not_null())
            .col(
                ColumnDef::new(Alias::new("horizon_days"))
                    .integer()
                    .not_null()
                    .default(7),
            )
            .col(ColumnDef::new(Alias::new("guidance")).text())
            .col(ColumnDef::new(Alias::new("nutrition")).text())
            .foreign_key(
                ForeignKey::create()
                    .from(Alias::new("plans"), Alias::new("checkin_id"))
                    .to(Alias::new("checkins"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::SetNull),
            );
        timestamps(&mut t);
        manager.create_table(t).await?;

        // plan_items
        let mut t = Table::create();
        t.table(Alias::new("plan_items")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(ColumnDef::new(Alias::new("plan_id")).integer().not_null())
            .col(ColumnDef::new(Alias::new("kind")).text().not_null())
            .col(ColumnDef::new(Alias::new("title")).text().not_null())
            .col(ColumnDef::new(Alias::new("detail")).text())
            .col(ColumnDef::new(Alias::new("scheduled_for")).date())
            .foreign_key(
                ForeignKey::create()
                    .from(Alias::new("plan_items"), Alias::new("plan_id"))
                    .to(Alias::new("plans"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            );
        timestamps(&mut t);
        manager.create_table(t).await?;

        // plan_item_outcomes
        let mut t = Table::create();
        t.table(Alias::new("plan_item_outcomes")).if_not_exists();
        pk(&mut t, Alias::new("id"));
        t.col(
            ColumnDef::new(Alias::new("plan_item_id"))
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(Alias::new("status")).text().not_null())
        .col(ColumnDef::new(Alias::new("note")).text())
        .col(
            ColumnDef::new(Alias::new("recorded_at"))
                .timestamp_with_time_zone()
                .not_null(),
        )
        .foreign_key(
            ForeignKey::create()
                .from(Alias::new("plan_item_outcomes"), Alias::new("plan_item_id"))
                .to(Alias::new("plan_items"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        );
        timestamps(&mut t);
        manager.create_table(t).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "plan_item_outcomes",
            "plan_items",
            "plans",
            "checkin_responses",
            "checkins",
            "observations",
            "protocols",
            "goals",
            "concern_tags",
            "concerns",
            "profile",
        ] {
            manager
                .drop_table(
                    Table::drop()
                        .table(Alias::new(table))
                        .if_exists()
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
