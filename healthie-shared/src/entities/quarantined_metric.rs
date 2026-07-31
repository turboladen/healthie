//! Verbatim HAE data points whose metric name is neither curated nor explicitly
//! excluded (ADR-0002 "never silently dropped", ADR-0005). Write-once per
//! `(raw_name, date)` — the durable discovery surface for metrics Apple/HAE add
//! later. Because curation is broad and declines are explicit, landing a row
//! here is exceptional, not routine.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "quarantined_metric")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// The unrecognized HAE metric name (`snake_case`, as received).
    pub raw_name: String,
    /// The local calendar day parsed from the point (never UTC-shifted).
    pub date: Date,
    /// The entire HAE data point, verbatim (`serde_json::Value`).
    #[sea_orm(column_type = "Json")]
    pub raw_point: Json,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

    use crate::{
        entities::quarantined_metric::{self},
        test_support::{date, datetime, test_db},
    };

    #[tokio::test]
    async fn quarantined_metric_round_trips_json_and_unique_name_date() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        let point = serde_json::json!({
            "date": "2026-07-28 00:00:00 -0700",
            "qty": 42.0,
            "source": "Future Apple Metric",
        });
        quarantined_metric::ActiveModel {
            raw_name: Set("some_future_metric".to_owned()),
            date: Set(date("2026-07-28")),
            raw_point: Set(point.clone()),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert");

        // Explicit end-to-end proof the Json column round-trips content, not
        // just row count (no other entity exercises a Json column).
        let found = quarantined_metric::Entity::find()
            .all(&db)
            .await
            .expect("q");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].raw_name, "some_future_metric");
        assert_eq!(found[0].raw_point, point);

        // UNIQUE(raw_name, date): same name+date is rejected.
        let dup = quarantined_metric::ActiveModel {
            raw_name: Set("some_future_metric".to_owned()),
            date: Set(date("2026-07-28")),
            raw_point: Set(serde_json::json!({ "other": true })),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await;
        assert!(dup.is_err(), "duplicate (raw_name,date) must be rejected");
    }
}
