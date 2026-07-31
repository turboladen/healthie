//! One curated health metric per `(kind, date)` local calendar day (ADR-0005).
//! Populated by the HAE ingest service (`services::metrics`): scalars store
//! `qty` in `value`; aggregates store `Avg`/`Min`/`Max` in `value`/`min`/`max`;
//! `sleep_analysis` explodes into one scalar row per stage. `UNIQUE(kind, date)`
//! makes ingest idempotent (last-write-wins). The unit is derived from `kind`
//! (`MetricKind::unit`), never stored.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "daily_metric")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub kind: MetricKind,
    /// The local calendar day the metric belongs to (never UTC-shifted).
    pub date: Date,
    /// Canonical daily number: `qty` for scalars, `Avg` for aggregates, the
    /// stage-hours for exploded sleep.
    pub value: f64,
    /// Populated only for aggregate metrics (e.g. `heart_rate`).
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Device string, informational (e.g. "Apple Watch"). Absent for many points.
    pub source: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

/// Closed vocabulary of curated metrics (ADR-0003 style). 25 variants: 19
/// scalar/aggregate kinds plus 6 sleep sub-metrics that `sleep_analysis`
/// explodes into. `MetricKind::iter()` enumerates the legal values.
///
/// `Hash`/`Ord` exist for the Apple Health backfill, which keys its per-day
/// rollup accumulator on `(MetricKind, NaiveDate)` and orders its report by
/// kind. `Ord` follows declaration order, which reads sensibly in that report.
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum MetricKind {
    #[sea_orm(string_value = "weight")]
    #[serde(rename = "weight")]
    Weight,
    #[sea_orm(string_value = "body-fat")]
    #[serde(rename = "body-fat")]
    BodyFat,
    #[sea_orm(string_value = "vo2-max")]
    #[serde(rename = "vo2-max")]
    Vo2Max,
    #[sea_orm(string_value = "resting-heart-rate")]
    #[serde(rename = "resting-heart-rate")]
    RestingHeartRate,
    #[sea_orm(string_value = "heart-rate")]
    #[serde(rename = "heart-rate")]
    HeartRate,
    #[sea_orm(string_value = "hrv")]
    #[serde(rename = "hrv")]
    Hrv,
    #[sea_orm(string_value = "spo2")]
    #[serde(rename = "spo2")]
    Spo2,
    #[sea_orm(string_value = "breathing-disturbances")]
    #[serde(rename = "breathing-disturbances")]
    BreathingDisturbances,
    #[sea_orm(string_value = "respiratory-rate")]
    #[serde(rename = "respiratory-rate")]
    RespiratoryRate,
    #[sea_orm(string_value = "cardio-recovery")]
    #[serde(rename = "cardio-recovery")]
    CardioRecovery,
    #[sea_orm(string_value = "active-energy")]
    #[serde(rename = "active-energy")]
    ActiveEnergy,
    #[sea_orm(string_value = "steps")]
    #[serde(rename = "steps")]
    Steps,
    #[sea_orm(string_value = "exercise-minutes")]
    #[serde(rename = "exercise-minutes")]
    ExerciseMinutes,
    #[sea_orm(string_value = "walking-distance")]
    #[serde(rename = "walking-distance")]
    WalkingDistance,
    #[sea_orm(string_value = "stand-minutes")]
    #[serde(rename = "stand-minutes")]
    StandMinutes,
    #[sea_orm(string_value = "walking-speed")]
    #[serde(rename = "walking-speed")]
    WalkingSpeed,
    #[sea_orm(string_value = "gait-asymmetry")]
    #[serde(rename = "gait-asymmetry")]
    GaitAsymmetry,
    #[sea_orm(string_value = "gait-double-support")]
    #[serde(rename = "gait-double-support")]
    GaitDoubleSupport,
    #[sea_orm(string_value = "step-length")]
    #[serde(rename = "step-length")]
    StepLength,
    #[sea_orm(string_value = "sleep-total")]
    #[serde(rename = "sleep-total")]
    SleepTotal,
    #[sea_orm(string_value = "sleep-deep")]
    #[serde(rename = "sleep-deep")]
    SleepDeep,
    #[sea_orm(string_value = "sleep-rem")]
    #[serde(rename = "sleep-rem")]
    SleepRem,
    #[sea_orm(string_value = "sleep-core")]
    #[serde(rename = "sleep-core")]
    SleepCore,
    #[sea_orm(string_value = "sleep-awake")]
    #[serde(rename = "sleep-awake")]
    SleepAwake,
    #[sea_orm(string_value = "time-in-bed")]
    #[serde(rename = "time-in-bed")]
    TimeInBed,
}

impl MetricKind {
    /// Canonical unit for this metric, derived from the kind (never stored).
    /// Ingest logs a warning when HAE's `units` string disagrees. The match is
    /// exhaustive with no wildcard on purpose: a new variant fails to compile
    /// until it declares a unit.
    #[must_use]
    pub fn unit(self) -> &'static str {
        match self {
            Self::Weight => "lb",
            Self::BodyFat | Self::GaitAsymmetry | Self::GaitDoubleSupport | Self::Spo2 => "%",
            Self::Vo2Max => "ml/(kg·min)",
            Self::RestingHeartRate
            | Self::HeartRate
            | Self::CardioRecovery
            | Self::RespiratoryRate => "count/min",
            Self::Hrv => "ms",
            Self::BreathingDisturbances | Self::Steps => "count",
            Self::ActiveEnergy => "kcal",
            Self::ExerciseMinutes | Self::StandMinutes => "min",
            Self::WalkingDistance => "mi",
            Self::WalkingSpeed => "mi/hr",
            Self::StepLength => "in",
            Self::SleepTotal
            | Self::SleepDeep
            | Self::SleepRem
            | Self::SleepCore
            | Self::SleepAwake
            | Self::TimeInBed => "hr",
        }
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

    use crate::{
        entities::daily_metric::{self, MetricKind},
        test_support::{date, datetime, test_db},
    };

    #[tokio::test]
    async fn daily_metric_round_trips_with_min_max_and_unique_kind_date() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        daily_metric::ActiveModel {
            kind: Set(MetricKind::HeartRate),
            date: Set(date("2026-07-28")),
            value: Set(54.3),
            min: Set(Some(43.0)),
            max: Set(Some(101.0)),
            source: Set(Some("Apple Watch".to_owned())),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert");

        let found = daily_metric::Entity::find().all(&db).await.expect("q");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, MetricKind::HeartRate);
        assert!((found[0].min.unwrap() - 43.0).abs() < f64::EPSILON);

        // UNIQUE(kind, date): same kind+date is rejected.
        let dup = daily_metric::ActiveModel {
            kind: Set(MetricKind::HeartRate),
            date: Set(date("2026-07-28")),
            value: Set(60.0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await;
        assert!(dup.is_err(), "duplicate (kind,date) must be rejected");
    }
}
