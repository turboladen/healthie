//! HAE ingest (ADR-0005): map Health Auto Export metric names to curated
//! [`MetricKind`]s, explode `sleep_analysis` into per-stage sub-metrics, and
//! quarantine genuinely unknown names. All persistence flows through
//! [`ingest_hae`] in one transaction; the mapping/extraction helpers below are
//! pure so they can be unit-tested without a database.

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    TransactionTrait,
};
use serde::Deserialize;

use crate::{
    clock,
    entities::{
        daily_metric::{self, MetricKind},
        quarantined_metric,
    },
    error::DomainResult,
};

/// How an incoming HAE metric name is classified.
pub(crate) enum HaeMapping {
    /// A scalar/aggregate metric name → one [`MetricKind`].
    Curated(MetricKind),
    /// `sleep_analysis` → up to 6 rows via [`sleep_rows`].
    Sleep,
    /// Seen and deliberately not tracked (spec §2) — silently ignored.
    Excluded,
    /// Never mapped → quarantined, never dropped.
    Unknown,
}

/// HAE names we have seen and deliberately do not curate. Keeps quarantine
/// exceptional (only genuinely new names land there). Promote one by moving it
/// out of here into [`map_hae_name`]'s match with a new [`MetricKind`].
pub(crate) const EXCLUDED_HAE_NAMES: &[&str] = &[
    "apple_stand_hour",
    "basal_energy_burned",
    "physical_effort",
    "apple_sleeping_wrist_temperature",
    "time_in_daylight",
    "walking_heart_rate_average",
    "environmental_audio_exposure",
    "headphone_audio_exposure",
];

/// Classify an HAE metric `name` into curated / sleep / excluded / unknown.
pub(crate) fn map_hae_name(name: &str) -> HaeMapping {
    let kind = match name {
        "weight_body_mass" => MetricKind::Weight,
        "body_fat_percentage" => MetricKind::BodyFat,
        "vo2_max" => MetricKind::Vo2Max,
        "resting_heart_rate" => MetricKind::RestingHeartRate,
        "heart_rate" => MetricKind::HeartRate,
        "heart_rate_variability" => MetricKind::Hrv,
        "blood_oxygen_saturation" => MetricKind::Spo2,
        "breathing_disturbances" => MetricKind::BreathingDisturbances,
        "respiratory_rate" => MetricKind::RespiratoryRate,
        "cardio_recovery" => MetricKind::CardioRecovery,
        "active_energy" => MetricKind::ActiveEnergy,
        "step_count" => MetricKind::Steps,
        "apple_exercise_time" => MetricKind::ExerciseMinutes,
        "walking_running_distance" => MetricKind::WalkingDistance,
        "apple_stand_time" => MetricKind::StandMinutes,
        "walking_speed" => MetricKind::WalkingSpeed,
        "walking_asymmetry_percentage" => MetricKind::GaitAsymmetry,
        "walking_double_support_percentage" => MetricKind::GaitDoubleSupport,
        "walking_step_length" => MetricKind::StepLength,
        "sleep_analysis" => return HaeMapping::Sleep,
        other if EXCLUDED_HAE_NAMES.contains(&other) => return HaeMapping::Excluded,
        _ => return HaeMapping::Unknown,
    };
    HaeMapping::Curated(kind)
}

/// Explode a `sleep_analysis` point into `(kind, hours)` pairs. A stage field
/// that is absent produces no row (skipped, not zeroed).
pub(crate) fn sleep_rows(point: &serde_json::Value) -> Vec<(MetricKind, f64)> {
    const FIELDS: &[(&str, MetricKind)] = &[
        ("totalSleep", MetricKind::SleepTotal),
        ("deep", MetricKind::SleepDeep),
        ("rem", MetricKind::SleepRem),
        ("core", MetricKind::SleepCore),
        ("awake", MetricKind::SleepAwake),
        ("inBed", MetricKind::TimeInBed),
    ];
    FIELDS
        .iter()
        .filter_map(|(field, kind)| {
            point
                .get(field)
                .and_then(serde_json::Value::as_f64)
                .map(|v| (*kind, v))
        })
        .collect()
}

/// Lenient, `Deserialize`-only DTO for the HAE export envelope
/// (`{ data: { metrics: [...] } }`). Extra fields are ignored.
#[derive(Debug, Deserialize)]
pub struct HaePayload {
    pub data: HaeData,
}

#[derive(Debug, Deserialize)]
pub struct HaeData {
    pub metrics: Vec<HaeMetric>,
}

#[derive(Debug, Deserialize)]
pub struct HaeMetric {
    pub name: String,
    #[serde(default)]
    pub units: Option<String>,
    /// Raw points; shape (`qty` scalar / `Avg`-`Min`-`Max` aggregate /
    /// `sleep_analysis` structured) is resolved per point at ingest.
    #[serde(default)]
    pub data: Vec<serde_json::Value>,
}

/// Outcome summary of one ingest. The backend logs it (`tracing::info!`); the
/// quarantine rows are the durable record.
#[derive(Debug)]
pub struct IngestReport {
    pub ingested: usize,
    pub quarantined: Vec<String>,
    pub date_range: Option<(NaiveDate, NaiveDate)>,
}

/// Ingest one HAE health-metrics payload into `daily_metric` (curated) and
/// `quarantined_metric` (unknown names), in one transaction. Idempotent:
/// `(kind, date)` and `(raw_name, date)` upsert last-write-wins.
///
/// # Errors
/// Returns [`crate::error::DomainError::Db`] on database errors. Malformed
/// individual points (missing/unparseable `date`, no usable value) are skipped,
/// not fatal — a partial payload still lands what it can.
pub async fn ingest_hae<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    payload: HaePayload,
) -> DomainResult<IngestReport> {
    let txn = db.begin().await?;
    let now = clock::now();
    let mut ingested = 0usize;
    let mut quarantined = Vec::new();
    let mut min_date: Option<NaiveDate> = None;
    let mut max_date: Option<NaiveDate> = None;

    for metric in payload.data.metrics {
        match map_hae_name(&metric.name) {
            HaeMapping::Excluded => {}
            HaeMapping::Unknown => {
                let mut any = false;
                for point in &metric.data {
                    let Some(date) = point_date(point) else {
                        continue;
                    };
                    upsert_quarantine(&txn, &metric.name, date, point, now).await?;
                    track_range(&mut min_date, &mut max_date, date);
                    any = true;
                }
                if any {
                    quarantined.push(metric.name.clone());
                }
            }
            HaeMapping::Curated(kind) => {
                warn_on_unit_mismatch(kind, metric.units.as_deref());
                for point in &metric.data {
                    let Some(date) = point_date(point) else {
                        continue;
                    };
                    let Some((value, min, max)) = scalar_or_aggregate(point) else {
                        continue;
                    };
                    upsert_metric(&txn, kind, date, value, min, max, point_source(point), now)
                        .await?;
                    track_range(&mut min_date, &mut max_date, date);
                    ingested += 1;
                }
            }
            HaeMapping::Sleep => {
                for point in &metric.data {
                    let Some(date) = point_date(point) else {
                        continue;
                    };
                    let src = point_source(point);
                    for (kind, value) in sleep_rows(point) {
                        upsert_metric(&txn, kind, date, value, None, None, src.clone(), now)
                            .await?;
                        ingested += 1;
                    }
                    track_range(&mut min_date, &mut max_date, date);
                }
            }
        }
    }

    txn.commit().await?;
    Ok(IngestReport {
        ingested,
        quarantined,
        date_range: min_date.zip(max_date),
    })
}

/// Parse the point's `date` into its LOCAL calendar day. HAE stamps a local
/// offset (e.g. `-0700`); the metric belongs to that local day, so we take
/// `date_naive()` on the `FixedOffset` value and never UTC-convert.
fn point_date(point: &serde_json::Value) -> Option<NaiveDate> {
    let s = point.get("date")?.as_str()?;
    DateTime::<FixedOffset>::parse_from_str(s, "%Y-%m-%d %H:%M:%S %z")
        .ok()
        .map(|dt| dt.date_naive())
}

fn point_source(point: &serde_json::Value) -> Option<String> {
    point
        .get("source")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Scalar `qty` → `(value, None, None)`; aggregate → `(Avg, Min, Max)`; neither
/// → `None` (point skipped).
fn scalar_or_aggregate(point: &serde_json::Value) -> Option<(f64, Option<f64>, Option<f64>)> {
    if let Some(qty) = point.get("qty").and_then(serde_json::Value::as_f64) {
        return Some((qty, None, None));
    }
    let avg = point.get("Avg").and_then(serde_json::Value::as_f64)?;
    let min = point.get("Min").and_then(serde_json::Value::as_f64);
    let max = point.get("Max").and_then(serde_json::Value::as_f64);
    Some((avg, min, max))
}

fn track_range(min: &mut Option<NaiveDate>, max: &mut Option<NaiveDate>, date: NaiveDate) {
    *min = Some(min.map_or(date, |m| m.min(date)));
    *max = Some(max.map_or(date, |m| m.max(date)));
}

fn warn_on_unit_mismatch(kind: MetricKind, hae_units: Option<&str>) {
    if let Some(u) = hae_units.filter(|u| *u != kind.unit()) {
        tracing::warn!(?kind, expected = kind.unit(), got = u, "HAE unit mismatch");
    }
}

/// Upsert one `daily_metric` row on `(kind, date)` (last-write-wins). Branches
/// `insert`/`update` explicitly — never `.save()` with a `Set` PK.
// A private helper threading one row's columns; the arg list mirrors the table.
#[allow(clippy::too_many_arguments)]
async fn upsert_metric<C: ConnectionTrait>(
    db: &C,
    kind: MetricKind,
    date: NaiveDate,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    source: Option<String>,
    now: DateTime<Utc>,
) -> DomainResult<()> {
    let existing = daily_metric::Entity::find()
        .filter(daily_metric::Column::Kind.eq(kind))
        .filter(daily_metric::Column::Date.eq(date))
        .one(db)
        .await?;
    match existing {
        Some(row) => {
            let mut active: daily_metric::ActiveModel = row.into();
            active.value = Set(value);
            active.min = Set(min);
            active.max = Set(max);
            active.source = Set(source);
            active.updated_at = Set(now);
            active.update(db).await?;
        }
        None => {
            daily_metric::ActiveModel {
                kind: Set(kind),
                date: Set(date),
                value: Set(value),
                min: Set(min),
                max: Set(max),
                source: Set(source),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
    }
    Ok(())
}

/// Upsert one `quarantined_metric` row on `(raw_name, date)`, overwriting the
/// verbatim point last-write-wins.
async fn upsert_quarantine<C: ConnectionTrait>(
    db: &C,
    raw_name: &str,
    date: NaiveDate,
    point: &serde_json::Value,
    now: DateTime<Utc>,
) -> DomainResult<()> {
    let existing = quarantined_metric::Entity::find()
        .filter(quarantined_metric::Column::RawName.eq(raw_name))
        .filter(quarantined_metric::Column::Date.eq(date))
        .one(db)
        .await?;
    match existing {
        Some(row) => {
            let mut active: quarantined_metric::ActiveModel = row.into();
            active.raw_point = Set(point.clone());
            active.update(db).await?;
        }
        None => {
            quarantined_metric::ActiveModel {
                raw_name: Set(raw_name.to_owned()),
                date: Set(date),
                raw_point: Set(point.clone()),
                created_at: Set(now),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sea_orm::EntityTrait;

    use super::{EXCLUDED_HAE_NAMES, HaeMapping, HaePayload, ingest_hae, map_hae_name, sleep_rows};
    use crate::{
        entities::{
            daily_metric::{self, MetricKind},
            quarantined_metric,
        },
        test_support::{date, test_db},
    };

    fn payload(metrics: serde_json::Value) -> HaePayload {
        // Build the envelope by hand so `metrics` is moved (json! would borrow).
        let mut data = serde_json::Map::new();
        data.insert("metrics".to_owned(), metrics);
        let mut root = serde_json::Map::new();
        root.insert("data".to_owned(), serde_json::Value::Object(data));
        serde_json::from_value(serde_json::Value::Object(root)).expect("valid HAE payload")
    }

    #[test]
    fn hae_names_map_to_curated_kinds() {
        assert!(matches!(
            map_hae_name("weight_body_mass"),
            HaeMapping::Curated(MetricKind::Weight)
        ));
        assert!(matches!(
            map_hae_name("heart_rate"),
            HaeMapping::Curated(MetricKind::HeartRate)
        ));
        assert!(matches!(
            map_hae_name("apple_stand_time"),
            HaeMapping::Curated(MetricKind::StandMinutes)
        ));
        assert!(matches!(map_hae_name("sleep_analysis"), HaeMapping::Sleep));
        assert!(matches!(
            map_hae_name("apple_stand_hour"),
            HaeMapping::Excluded
        ));
        assert!(matches!(
            map_hae_name("environmental_audio_exposure"),
            HaeMapping::Excluded
        ));
        assert!(matches!(
            map_hae_name("some_new_apple_metric_2027"),
            HaeMapping::Unknown
        ));
    }

    #[test]
    fn curated_and_excluded_sets_are_disjoint() {
        for name in EXCLUDED_HAE_NAMES {
            assert!(
                matches!(map_hae_name(name), HaeMapping::Excluded),
                "{name} is both excluded and curated"
            );
        }
    }

    #[test]
    fn every_curated_kind_has_a_unit() {
        use sea_orm::strum::IntoEnumIterator as _;
        for kind in MetricKind::iter() {
            assert!(!kind.unit().is_empty(), "{kind:?} needs a unit");
        }
    }

    #[test]
    fn sleep_rows_extracts_present_stages_only() {
        let point = serde_json::json!({
            "totalSleep": 6.1, "deep": 1.36, "rem": 1.34, "core": 3.4, "inBed": 6.55
            // "awake" absent → SleepAwake row omitted
        });
        let rows = sleep_rows(&point);
        let kinds: Vec<_> = rows.iter().map(|(k, _)| *k).collect();
        assert!(kinds.contains(&MetricKind::SleepTotal));
        assert!(kinds.contains(&MetricKind::TimeInBed));
        assert!(
            !kinds.contains(&MetricKind::SleepAwake),
            "absent field → no row"
        );
        let total = rows
            .iter()
            .find(|(k, _)| *k == MetricKind::SleepTotal)
            .unwrap()
            .1;
        assert!((total - 6.1).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn ingest_scalar_aggregate_and_sleep() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "weight_body_mass", "units": "lb",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 234.0, "source": "Scale" }] },
                // Late-evening local time so a UTC-convert bug would shift this
                // to 2026-07-29; the local day is 2026-07-28.
                { "name": "heart_rate", "units": "count/min",
                  "data": [{ "date": "2026-07-28 22:30:00 -0700", "Avg": 54.3, "Min": 43.0, "Max": 101.0 }] },
                { "name": "sleep_analysis", "units": "hr",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "totalSleep": 6.1, "deep": 1.3, "rem": 1.3, "core": 3.4, "inBed": 6.5 }] }
            ])),
        )
        .await
        .expect("ingest");

        // weight(1) + heart_rate(1) + sleep(5 present stages) = 7 rows.
        assert_eq!(report.ingested, 7);
        assert!(report.quarantined.is_empty());
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 7);
        let hr = rows
            .iter()
            .find(|r| r.kind == MetricKind::HeartRate)
            .unwrap();
        assert_eq!((hr.min, hr.max), (Some(43.0), Some(101.0)));
        assert!((hr.value - 54.3).abs() < f64::EPSILON);
        // Local date is the -0700 calendar day, not UTC-shifted to 07-29.
        assert_eq!(hr.date, date("2026-07-28"));
    }

    #[tokio::test]
    async fn unknown_quarantined_excluded_ignored() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "apple_stand_hour", "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 15.0 }] },
                { "name": "brand_new_metric", "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 9.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        assert_eq!(report.ingested, 0);
        assert_eq!(report.quarantined, vec!["brand_new_metric".to_owned()]);
        assert!(
            daily_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            quarantined_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn upsert_is_idempotent_last_write_wins() {
        let db = test_db().await;
        let one = |v: f64| {
            payload(serde_json::json!([
                { "name": "weight_body_mass", "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": v }] }
            ]))
        };
        ingest_hae(&db, one(234.0)).await.expect("first");
        ingest_hae(&db, one(232.5)).await.expect("second");
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 1, "same (kind,date) upserts, not duplicates");
        assert!(
            (rows[0].value - 232.5).abs() < f64::EPSILON,
            "last write wins"
        );
    }

    #[tokio::test]
    async fn empty_data_metric_is_not_reported_quarantined() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "brand_new_metric", "data": [] }
            ])),
        )
        .await
        .expect("ingest");
        assert_eq!(report.ingested, 0);
        assert!(
            report.quarantined.is_empty(),
            "a metric with no landable points must not appear in the report"
        );
        assert!(
            quarantined_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn point_missing_date_is_skipped() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "weight_body_mass", "data": [{ "qty": 200.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        assert_eq!(report.ingested, 0);
        assert!(
            daily_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn aggregate_without_min_max_stores_avg_only() {
        let db = test_db().await;
        ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "heart_rate", "data": [{ "date": "2026-07-28 00:00:00 -0700", "Avg": 61.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].value - 61.0).abs() < f64::EPSILON);
        assert_eq!(rows[0].min, None);
        assert_eq!(rows[0].max, None);
    }
}
