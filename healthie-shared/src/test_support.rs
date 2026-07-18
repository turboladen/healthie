use chrono::{DateTime, NaiveDate, Utc};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::migration::Migrator;

/// Parse a `YYYY-MM-DD` string into a typed `NaiveDate` for test seeds/asserts.
///
/// All datetime writers go through the sqlx `DateTime<Utc>` encoder; this helper
/// keeps tests on typed values rather than raw strings.
///
/// # Panics
/// Panics if `s` is not a valid `YYYY-MM-DD` date.
#[must_use]
pub fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("valid YYYY-MM-DD date literal")
}

/// Parse a `YYYY-MM-DD HH:MM:SS` string into a UTC `DateTime` for test
/// seeds/asserts.
///
/// # Panics
/// Panics if `s` is not a valid `YYYY-MM-DD HH:MM:SS` datetime.
#[must_use]
pub fn datetime(s: &str) -> DateTime<Utc> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .expect("valid YYYY-MM-DD HH:MM:SS datetime literal")
        .and_utc()
}

/// In-memory `SQLite`, single pinned connection, fully migrated.
///
/// # Panics
/// Panics if the in-memory database cannot be opened or the migrations fail.
pub async fn test_db() -> DatabaseConnection {
    let mut opt = ConnectOptions::new("sqlite::memory:");
    opt.max_connections(1)
        .min_connections(1)
        .sqlx_logging(false)
        // Explicit, not default-reliant: FKs are declared in the schema and
        // must be enforced on every connection (healthie-38x).
        .map_sqlx_sqlite_opts(|o| o.foreign_keys(true));
    let db = Database::connect(opt)
        .await
        .expect("connect in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    db
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Statement};

    /// FKs are declared in the schema but only enforced if the connection has
    /// `PRAGMA foreign_keys=ON`. Services self-enforce via `require()`, but the
    /// pragma is the backstop — prove it's live on test connections.
    #[tokio::test]
    async fn test_db_enforces_foreign_keys() {
        let db = super::test_db().await;
        let result = db
            .execute(Statement::from_string(
                db.get_database_backend(),
                // concern_tags.concern_id -> concerns.id; 9999 doesn't exist.
                "INSERT INTO concern_tags (concern_id, tag, created_at, updated_at) VALUES (9999, \
                 'general', '2026-07-17T00:00:00Z', '2026-07-17T00:00:00Z')"
                    .to_owned(),
            ))
            .await;
        assert!(
            result.is_err(),
            "FK violation must be rejected — PRAGMA foreign_keys is not enforced"
        );
    }

    #[tokio::test]
    async fn migrations_create_all_m1_tables() {
        let db = super::test_db().await;
        for table in [
            "profile",
            "concerns",
            "concern_tags",
            "goals",
            "protocols",
            "observations",
            "checkins",
            "checkin_responses",
            "plans",
            "plan_items",
            "plan_item_outcomes",
        ] {
            let row = db
                .query_one(Statement::from_string(
                    db.get_database_backend(),
                    format!("SELECT name FROM sqlite_master WHERE type='table' AND name='{table}'"),
                ))
                .await
                .unwrap();
            assert!(row.is_some(), "missing table {table}");
        }
    }

    #[tokio::test]
    async fn entities_match_schema() {
        use sea_orm::EntityTrait;
        let db = super::test_db().await;
        // A find() per entity decodes zero rows from empty tables: it proves the
        // column names/types line up with the migration, not value round-trips —
        // those are covered by the per-service insert-then-read tests.
        crate::entities::profile::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::concern::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::concern_tag::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::goal::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::protocol::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::observation::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::checkin::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::checkin_response::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::plan::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::plan_item::Entity::find()
            .all(&db)
            .await
            .unwrap();
        crate::entities::plan_item_outcome::Entity::find()
            .all(&db)
            .await
            .unwrap();
    }
}
