use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::migration::Migrator;

/// In-memory `SQLite`, single pinned connection, fully migrated.
pub async fn test_db() -> DatabaseConnection {
    let mut opt = ConnectOptions::new("sqlite::memory:");
    opt.max_connections(1).min_connections(1).sqlx_logging(false);
    let db = Database::connect(opt)
        .await
        .expect("connect in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    db
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Statement};

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
                    format!(
                        "SELECT name FROM sqlite_master WHERE type='table' AND name='{table}'"
                    ),
                ))
                .await
                .unwrap();
            assert!(row.is_some(), "missing table {table}");
        }
    }
}
