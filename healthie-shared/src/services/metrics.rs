//! HAE ingest (ADR-0005, ADR-0007): map Health Auto Export metric names to
//! curated [`MetricKind`]s, explode `sleep_analysis` into per-stage
//! sub-metrics, convert every value into the kind's canonical unit, and refuse
//! what cannot be stored.
//!
//! Three things are quarantined rather than stored, and none of them is
//! silent: a name never seen before, a point whose unit is missing or outside
//! the vocabulary, and a value outside what the kind can physically be. The
//! refused point is held verbatim because this path has no file on disk to
//! re-read — the POST body is gone once the handler returns.
//!
//! All persistence flows through [`ingest_hae`] in one transaction; the
//! mapping/extraction helpers below are pure so they can be unit-tested
//! without a database.

use std::collections::{BTreeMap, BTreeSet};

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
        quarantined_metric::{self, QuarantineMeta, QuarantineReason},
    },
    error::DomainResult,
    services::{
        plausibility,
        units::{Producer, convert_to_canonical},
    },
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

/// The HAE metric name that carries per-stage sleep, exploded by [`sleep_rows`].
pub(crate) const HAE_SLEEP_ANALYSIS: &str = "sleep_analysis";

/// Every HAE metric name this build curates, and the [`MetricKind`] it lands
/// on.
///
/// A table rather than a `match` so the live vocabulary is **enumerable**. That
/// matters for one specific guarantee: the Apple Health backfill's `HK_METRICS`
/// declares which kinds have no HAE counterpart at all, and only an enumerable
/// list lets a test assert that no HAE name resolves to one of them. A `match`
/// can only answer questions about names you already thought to ask.
pub(crate) const CURATED_HAE_NAMES: &[(&str, MetricKind)] = &[
    ("weight_body_mass", MetricKind::Weight),
    ("body_fat_percentage", MetricKind::BodyFat),
    ("vo2_max", MetricKind::Vo2Max),
    ("resting_heart_rate", MetricKind::RestingHeartRate),
    ("heart_rate", MetricKind::HeartRate),
    ("heart_rate_variability", MetricKind::Hrv),
    ("blood_oxygen_saturation", MetricKind::Spo2),
    ("breathing_disturbances", MetricKind::BreathingDisturbances),
    ("respiratory_rate", MetricKind::RespiratoryRate),
    ("cardio_recovery", MetricKind::CardioRecovery),
    ("active_energy", MetricKind::ActiveEnergy),
    ("step_count", MetricKind::Steps),
    ("apple_exercise_time", MetricKind::ExerciseMinutes),
    ("walking_running_distance", MetricKind::WalkingDistance),
    ("apple_stand_time", MetricKind::StandMinutes),
    ("flights_climbed", MetricKind::FlightsClimbed),
    ("walking_speed", MetricKind::WalkingSpeed),
    ("walking_asymmetry_percentage", MetricKind::GaitAsymmetry),
    (
        "walking_double_support_percentage",
        MetricKind::GaitDoubleSupport,
    ),
    ("walking_step_length", MetricKind::StepLength),
];

/// Classify an HAE metric `name` into curated / sleep / excluded / unknown.
pub(crate) fn map_hae_name(name: &str) -> HaeMapping {
    if name == HAE_SLEEP_ANALYSIS {
        return HaeMapping::Sleep;
    }
    if let Some((_, kind)) = CURATED_HAE_NAMES.iter().find(|(n, _)| *n == name) {
        return HaeMapping::Curated(*kind);
    }
    if EXCLUDED_HAE_NAMES.contains(&name) {
        return HaeMapping::Excluded;
    }
    HaeMapping::Unknown
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
    /// Names that were never recognized at all. Unchanged in meaning: a
    /// *curated* name that could not be stored appears in [`Self::refused`].
    pub quarantined: Vec<String>,
    pub date_range: Option<(NaiveDate, NaiveDate)>,
    /// Curated points this push would not store, and why. Every entry has a
    /// matching `quarantined_metric` row holding the point verbatim.
    ///
    /// Bounded by the points in one push. It is the thing to read after a
    /// deploy: a name appearing here every day means the live feed has changed
    /// shape and rows that used to land are now being held instead.
    pub refused: Vec<Refusal>,
    /// Rows that stored, but with an `min`/`max` column dropped because that
    /// number alone could not be trusted. The row's `value` is unaffected.
    pub bounds_cleared: usize,
    /// Standing quarantine rows deleted because their `(name, date)` stored
    /// cleanly this time. Reported because this push DELETES those rows, and a
    /// run that silently retired forty complaints should not look identical to
    /// one that retired none.
    pub quarantine_retired: usize,
}

/// One curated point that could not be stored as it arrived.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub name: String,
    pub date: NaiveDate,
    pub reason: QuarantineReason,
    /// The kinds refused. Several when one `sleep_analysis` point explodes and
    /// only some stages were impossible.
    pub kinds: Vec<MetricKind>,
    /// Whether a row still landed for this `(name, date)` — true when a bound
    /// was dropped rather than the point refused. A day carrying both outcomes
    /// reports `true`, because a row is in fact there.
    pub stored: bool,
}

/// A refusal accumulated per `(raw_name, date)`.
///
/// Keyed and resolved *after* the point loop rather than written inside it,
/// because `quarantined_metric` is keyed `(raw_name, date)` while refusals are
/// per point: two points on one date would otherwise let array order decide
/// whether the day ends up complained about or swept clean.
struct Complaint {
    point: serde_json::Value,
    reason: QuarantineReason,
    units: Option<String>,
    kinds: Vec<MetricKind>,
    stored: bool,
}

/// Ingest one HAE health-metrics payload into `daily_metric` (curated) and
/// `quarantined_metric` (unknown names, and curated points that could not be
/// stored), in one transaction. Idempotent: `(kind, date)` and
/// `(raw_name, date)` upsert last-write-wins.
///
/// # What it refuses, and why nothing is lost
///
/// Values are converted to `MetricKind::unit()` before storage and refused
/// when they cannot be — the live counterpart of ADR-0006 §6, and the reason
/// a `kg`-declared weight is no longer stored as though it were already
/// pounds. A refused point is written verbatim to `quarantined_metric` with
/// the declared units and the reason, because unlike the backfill there is no
/// file on disk to re-read: the POST body is gone when this returns.
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
    let mut run = Ingest::default();

    for metric in payload.data.metrics {
        match map_hae_name(&metric.name) {
            HaeMapping::Excluded => {}
            HaeMapping::Unknown => run.unknown(&metric),
            HaeMapping::Curated(kind) => run.curated(&txn, kind, &metric, now).await?,
            HaeMapping::Sleep => run.sleep(&txn, &metric, now).await?,
        }
    }

    let report = run.finish(&txn, now).await?;
    txn.commit().await?;
    Ok(report)
}

/// The mutable state of one ingest, so each arm is its own readable unit and
/// the counters cannot drift apart across them.
#[derive(Default)]
struct Ingest {
    ingested: usize,
    quarantined: Vec<String>,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    complaints: BTreeMap<(String, NaiveDate), Complaint>,
    /// Keys that stored something cleanly. Only those with no complaint at all
    /// retire a standing quarantine row.
    settled: BTreeSet<(String, NaiveDate)>,
    bounds_cleared: usize,
}

impl Ingest {
    /// A name never seen before: every point of it is held verbatim.
    fn unknown(&mut self, metric: &HaeMetric) {
        let mut any = false;
        for point in &metric.data {
            let Some(date) = point_date(point) else {
                continue;
            };
            self.complain(
                (&metric.name, date),
                point,
                QuarantineReason::UnknownType,
                metric.units.as_deref(),
                None,
            );
            self.track_range(date);
            any = true;
        }
        if any {
            self.quarantined.push(metric.name.clone());
        }
    }

    /// A curated scalar or aggregate metric.
    async fn curated<C: ConnectionTrait>(
        &mut self,
        db: &C,
        kind: MetricKind,
        metric: &HaeMetric,
        now: DateTime<Utc>,
    ) -> DomainResult<()> {
        for point in &metric.data {
            let Some(date) = point_date(point) else {
                continue;
            };
            let Some((value, min, max)) = scalar_or_aggregate(point) else {
                continue;
            };
            self.track_range(date);
            let key = (metric.name.as_str(), date);
            // Units are declared once per metric, not per point, so a metric
            // with none refuses every point it carries rather than assuming
            // they arrived canonical.
            let Some(units) = metric.units.as_deref() else {
                self.complain(key, point, QuarantineReason::MissingUnit, None, Some(kind));
                continue;
            };
            let value = match canonical(units, kind, value) {
                Ok(value) => value,
                Err(reason) => {
                    self.complain(key, point, reason, Some(units), Some(kind));
                    continue;
                }
            };
            warn_if_percent_looks_like_a_fraction(kind, value);
            // A bound we cannot trust is not written, but it does not cost the
            // day its average: the row lands without it, and the point is
            // still held verbatim so the dropped number is recoverable.
            let (min, min_refused) = canonical_bound(units, kind, min);
            let (max, max_refused) = canonical_bound(units, kind, max);
            if let Some(reason) = min_refused.or(max_refused) {
                self.bounds_cleared += 1;
                self.complain_stored(key, point, reason, Some(units), Some(kind));
            } else {
                self.settled.insert((metric.name.clone(), date));
            }
            upsert_metric(db, kind, date, value, min, max, point_source(point), now).await?;
            self.ingested += 1;
        }
        Ok(())
    }

    /// `sleep_analysis`, which explodes one point into up to six rows.
    async fn sleep<C: ConnectionTrait>(
        &mut self,
        db: &C,
        metric: &HaeMetric,
        now: DateTime<Utc>,
    ) -> DomainResult<()> {
        for point in &metric.data {
            let Some(date) = point_date(point) else {
                continue;
            };
            let src = point_source(point);
            self.track_range(date);
            let key = (metric.name.as_str(), date);
            for (kind, raw) in sleep_rows(point) {
                // Refusal is per STAGE, not per point: each stage is its own
                // `(kind, date)` row, and a refused one is the same shape as a
                // stage field that was simply absent (ADR-0005 §3). One
                // impossible total must not discard the ordinary deep/REM/core
                // rows recorded beside it.
                let converted = match metric.units.as_deref() {
                    None => Err(QuarantineReason::MissingUnit),
                    Some(units) => canonical(units, kind, raw),
                };
                match converted {
                    Ok(value) => {
                        upsert_metric(db, kind, date, value, None, None, src.clone(), now).await?;
                        self.ingested += 1;
                        self.settled.insert((metric.name.clone(), date));
                    }
                    Err(reason) => {
                        self.complain(key, point, reason, metric.units.as_deref(), Some(kind));
                    }
                }
            }
        }
        Ok(())
    }

    /// Write the complaints, retire the resolved ones, and describe the run.
    async fn finish<C: ConnectionTrait>(
        self,
        db: &C,
        now: DateTime<Utc>,
    ) -> DomainResult<IngestReport> {
        let refused = persist_complaints(db, &self.complaints, now).await?;
        // Only keys with no complaint at all: a day that stored five sleep
        // stages and refused one still has something to say.
        let mut quarantine_retired = 0u64;
        for (name, date) in &self.settled {
            if !self.complaints.contains_key(&(name.clone(), *date)) {
                quarantine_retired += clear_quarantine(db, name, *date).await?;
            }
        }
        Ok(IngestReport {
            ingested: self.ingested,
            quarantined: self.quarantined,
            date_range: self.min_date.zip(self.max_date),
            refused,
            bounds_cleared: self.bounds_cleared,
            quarantine_retired: usize::try_from(quarantine_retired).unwrap_or(usize::MAX),
        })
    }

    fn track_range(&mut self, date: NaiveDate) {
        self.min_date = Some(self.min_date.map_or(date, |m| m.min(date)));
        self.max_date = Some(self.max_date.map_or(date, |m| m.max(date)));
    }

    fn complain(
        &mut self,
        key: (&str, NaiveDate),
        point: &serde_json::Value,
        reason: QuarantineReason,
        units: Option<&str>,
        kind: Option<MetricKind>,
    ) {
        complain(&mut self.complaints, key, point, reason, units, kind);
    }

    /// [`Self::complain`] for a point that still stored a row — only a bound
    /// was dropped.
    ///
    /// `stored` latches on regardless of what else this key has recorded. A
    /// day can carry both a fully refused point and a partially stored one,
    /// and a row genuinely landed, so reporting otherwise would contradict the
    /// database over which of two points happened to come first in the array.
    fn complain_stored(
        &mut self,
        key: (&str, NaiveDate),
        point: &serde_json::Value,
        reason: QuarantineReason,
        units: Option<&str>,
        kind: Option<MetricKind>,
    ) {
        complain(&mut self.complaints, key, point, reason, units, kind).stored = true;
    }
}

/// Record that `(name, date)` could not be fully stored, returning the running
/// complaint. Merges into whatever this key already holds, so a point refused
/// for several kinds lands one row.
fn complain<'a>(
    complaints: &'a mut BTreeMap<(String, NaiveDate), Complaint>,
    (name, date): (&str, NaiveDate),
    point: &serde_json::Value,
    reason: QuarantineReason,
    units: Option<&str>,
    kind: Option<MetricKind>,
) -> &'a mut Complaint {
    let entry = complaints
        .entry((name.to_owned(), date))
        .or_insert_with(|| Complaint {
            point: point.clone(),
            reason,
            units: units.map(str::to_owned),
            kinds: Vec::new(),
            stored: false,
        });
    if let Some(kind) = kind
        && !entry.kinds.contains(&kind)
    {
        entry.kinds.push(kind);
    }
    entry
}

/// Write every accumulated complaint, and describe them for the report.
async fn persist_complaints<C: ConnectionTrait>(
    db: &C,
    complaints: &BTreeMap<(String, NaiveDate), Complaint>,
    now: DateTime<Utc>,
) -> DomainResult<Vec<Refusal>> {
    let mut refused = Vec::new();
    for ((name, date), complaint) in complaints {
        upsert_quarantine(
            db,
            name,
            *date,
            &complaint.point,
            QuarantineMeta {
                reason: complaint.reason,
                units: complaint.units.as_deref(),
                kinds: &complaint.kinds,
            },
            now,
        )
        .await?;
        // An unknown NAME is already reported by name; listing it again as a
        // refused point would double-count the same row.
        if !complaint.kinds.is_empty() {
            refused.push(Refusal {
                name: name.clone(),
                date: *date,
                reason: complaint.reason,
                kinds: complaint.kinds.clone(),
                stored: complaint.stored,
            });
        }
    }
    Ok(refused)
}

/// Convert one number into `kind`'s canonical unit, or say why it cannot be.
fn canonical(units: &str, kind: MetricKind, raw: f64) -> Result<f64, QuarantineReason> {
    // Before conversion, which would happily pass a NaN through its fast path.
    //
    // Not reachable across the wire today — `serde_json` rejects a non-finite
    // literal at parse and `Number::from_f64` refuses to build one — so this
    // is defense in depth against an in-process caller or a future
    // `arbitrary_precision`, not a live defect. `backend_integration_test`
    // pins the wire-level property this rests on.
    if !raw.is_finite() {
        return Err(QuarantineReason::NonFiniteValue);
    }
    let value = convert_to_canonical(units, kind, raw, Producer::HealthAutoExport)
        .ok_or(QuarantineReason::UnconvertibleUnit)?;
    // Bounds are stated in the canonical unit, so they can only be applied
    // after conversion: 100 kg is a plausible weight and 100 lb is too, but
    // only one of them is what this row would have meant.
    match plausibility::reject_reason(kind, value) {
        Some(reason) => Err(reason),
        None => Ok(value),
    }
}

/// [`canonical`] for an optional `min`/`max` column: absent stays absent, and
/// a bound that will not convert is dropped rather than stored raw.
fn canonical_bound(
    units: &str,
    kind: MetricKind,
    raw: Option<f64>,
) -> (Option<f64>, Option<QuarantineReason>) {
    match raw.map(|raw| canonical(units, kind, raw)) {
        None => (None, None),
        Some(Ok(value)) => (Some(value), None),
        Some(Err(reason)) => (None, Some(reason)),
    }
}

/// The live path's counterpart to the import report's fraction tripwire.
///
/// HAE's percent convention is UNVERIFIED (healthie-t58) and
/// [`Producer::HealthAutoExport`] assumes 0-100, which preserves what this path
/// did before it converted at all. If that assumption is wrong the stored
/// number is 100x low and *nothing else notices* — a `0.303` body fat is
/// comfortably inside every plausibility bound. So say so, on the first push,
/// where the answer will be sitting in the log.
fn warn_if_percent_looks_like_a_fraction(kind: MetricKind, value: f64) {
    if kind.unit() == "%" && value > 0.0 && value <= 1.0 {
        tracing::warn!(
            ?kind,
            value,
            "percent-typed metric arrived at or below 1.0 — HAE may be sending 0-1 fractions, \
             which this build does NOT scale (healthie-t58)"
        );
    }
}

/// Parse a `YYYY-MM-DD HH:MM:SS ±HHMM` stamp into an offset-aware instant.
///
/// Both metric intake paths — HAE's JSON `date` and the Apple Health
/// `export.xml` `startDate`/`endDate` attributes — use this identical wire
/// format, and both must interpret the offset the same way or a backfilled row
/// and a live row for the same reading would land on different days. One parse,
/// one format string, shared deliberately (ADR-0005 §5).
pub(crate) fn parse_local(s: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::<FixedOffset>::parse_from_str(s, "%Y-%m-%d %H:%M:%S %z").ok()
}

/// Parse the point's `date` into its LOCAL calendar day. HAE stamps a local
/// offset (e.g. `-0700`); the metric belongs to that local day, so we take
/// `date_naive()` on the `FixedOffset` value and never UTC-convert.
fn point_date(point: &serde_json::Value) -> Option<NaiveDate> {
    let s = point.get("date")?.as_str()?;
    parse_local(s).map(|dt| dt.date_naive())
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

/// Retire the complaint for `(raw_name, date)` once that key has stored
/// cleanly.
///
/// A quarantine row is a standing complaint about a specific day's reading. A
/// widened bound or a new unit spelling plus a re-POST resolves it, and upsert
/// never deletes — so without this, every recovery leaves litter behind
/// forever, and "quarantine stays exceptional" (ADR-0005 §4) stops being true.
///
/// A *persistent* cause still accrues one row per day, which is the complaint
/// working, not litter. Scoped by exact HAE name, so it can never reach an
/// `HK…` row the backfill wrote.
async fn clear_quarantine<C: ConnectionTrait>(
    db: &C,
    raw_name: &str,
    date: NaiveDate,
) -> DomainResult<u64> {
    let result = quarantined_metric::Entity::delete_many()
        .filter(quarantined_metric::Column::RawName.eq(raw_name))
        .filter(quarantined_metric::Column::Date.eq(date))
        .exec(db)
        .await?;
    // `rows_affected`, not the number of keys we asked about: on the one
    // operation here that destroys a row, the count reported is what actually
    // happened rather than what was intended.
    Ok(result.rows_affected)
}

/// Upsert one `daily_metric` row on `(kind, date)` (last-write-wins). Branches
/// `insert`/`update` explicitly — never `.save()` with a `Set` PK.
///
/// # A write replaces the WHOLE row, `min`/`max`/`source` included
///
/// So a later scalar push for a kind that previously landed an aggregate
/// clears the stored spread rather than keeping it. That is deliberate
/// (healthie-c47), and it is the safer of the two options rather than merely
/// the simpler one.
///
/// Coalescing — keeping a prior `min`/`max` when the incoming write has none —
/// would produce a **chimera row**: today's `value` from one intake beside a
/// spread computed months ago by another, under a `source` naming only one of
/// them. This is not hypothetical, because the `Mean` and `Sum` policies
/// resolve to `(value, None, None)` and the backfill writes those onto dates
/// where a live HAE aggregate may already sit. healthie-4lf.2 thresholds on
/// `min`/`max`; a stale bound paired with a fresh value is a silent lie with
/// nothing on the row to mark it.
///
/// A row is therefore exactly one intake's complete account of one day, never
/// a merge of two. ADR-0007 records this as a clarification of ADR-0005 §5,
/// which describes the row upserting and is silent about columns.
///
/// Returns the row as it stood **before** this write, or `None` if this was an
/// insert. The lookup happens either way, so returning it is free; the Apple
/// Health backfill uses it to report how far its reconstructed values diverge
/// from rows a live HAE push already landed, *before* overwriting them.
// A private helper threading one row's columns; the arg list mirrors the table.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn upsert_metric<C: ConnectionTrait>(
    db: &C,
    kind: MetricKind,
    date: NaiveDate,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    source: Option<String>,
    now: DateTime<Utc>,
) -> DomainResult<Option<daily_metric::Model>> {
    let existing = daily_metric::Entity::find()
        .filter(daily_metric::Column::Kind.eq(kind))
        .filter(daily_metric::Column::Date.eq(date))
        .one(db)
        .await?;
    if let Some(row) = existing {
        let prior = row.clone();
        let mut active: daily_metric::ActiveModel = row.into();
        active.value = Set(value);
        active.min = Set(min);
        active.max = Set(max);
        active.source = Set(source);
        active.updated_at = Set(now);
        active.update(db).await?;
        Ok(Some(prior))
    } else {
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
        Ok(None)
    }
}

/// Upsert one `quarantined_metric` row on `(raw_name, date)`, overwriting the
/// verbatim point last-write-wins.
///
/// `meta` is stamped into the point's `_import` object here rather than by the
/// caller, so a quarantine row with no recorded reason is unconstructible —
/// which matters now that a *curated* name can land here and the row is the
/// only thing that says why.
pub(crate) async fn upsert_quarantine<C: ConnectionTrait>(
    db: &C,
    raw_name: &str,
    date: NaiveDate,
    point: &serde_json::Value,
    meta: QuarantineMeta<'_>,
    now: DateTime<Utc>,
) -> DomainResult<()> {
    let mut point = point.clone();
    meta.stamp(&mut point);
    let existing = quarantined_metric::Entity::find()
        .filter(quarantined_metric::Column::RawName.eq(raw_name))
        .filter(quarantined_metric::Column::Date.eq(date))
        .one(db)
        .await?;
    match existing {
        Some(row) => {
            let mut active: quarantined_metric::ActiveModel = row.into();
            active.raw_point = Set(point);
            active.update(db).await?;
        }
        None => {
            quarantined_metric::ActiveModel {
                raw_name: Set(raw_name.to_owned()),
                date: Set(date),
                raw_point: Set(point),
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
            quarantined_metric::{self, QuarantineReason},
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
                { "name": "weight_body_mass", "units": "lb",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": v }] }
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

    /// healthie-c47: a write is one intake's WHOLE account of a day.
    ///
    /// A scalar push after an aggregate clears the spread rather than keeping
    /// it. Preserving it would pair today's value with a spread computed by
    /// some other run — a row that describes no single measurement, under a
    /// `source` naming only one of them, with nothing to mark it as a merge.
    #[tokio::test]
    async fn a_write_replaces_the_whole_row_not_only_the_columns_it_supplies() {
        let db = test_db().await;
        ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "heart_rate", "units": "count/min",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700",
                             "Avg": 61.0, "Min": 48.0, "Max": 130.0, "source": "Apple Watch" }] }
            ])),
        )
        .await
        .expect("aggregate");
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!((rows[0].min, rows[0].max), (Some(48.0), Some(130.0)));

        // A scalar for the same (kind, date) — the shape the backfill's Mean
        // and Sum policies produce, and a re-push of a day HAE rolled up
        // differently.
        ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "heart_rate", "units": "count/min",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 58.0 }] }
            ])),
        )
        .await
        .expect("scalar");

        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 1, "still one row for the day");
        assert!((rows[0].value - 58.0).abs() < f64::EPSILON);
        assert_eq!(
            (rows[0].min, rows[0].max),
            (None, None),
            "a spread this write did not measure must not survive under its value"
        );
        assert_eq!(
            rows[0].source, None,
            "and neither may the device credited with measuring it"
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
                { "name": "weight_body_mass", "units": "lb", "data": [{ "qty": 200.0 }] }
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

    /// healthie-ei8: the live path used to log a warning and store the number
    /// as though it had always been canonical. 100 kg stored as 100 lb is a
    /// 2.2x error no later reader can detect.
    #[tokio::test]
    async fn a_kg_declared_weight_is_converted_not_stored_raw() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "weight_body_mass", "units": "kg",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 100.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        assert_eq!(report.ingested, 1);
        assert!(report.refused.is_empty(), "kg converts, it does not refuse");
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert!(
            (rows[0].value - 220.462_262_184_877_6).abs() < 1e-9,
            "100 kg must land as pounds, got {}",
            rows[0].value
        );
    }

    /// An aggregate's spread is in the same declared unit as its average, and
    /// used to be stored untouched even when the average was converted.
    #[tokio::test]
    async fn aggregate_bounds_are_converted_with_the_value() {
        let db = test_db().await;
        ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "weight_body_mass", "units": "kg",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700",
                             "Avg": 100.0, "Min": 99.0, "Max": 101.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert!((rows[0].min.unwrap() - 218.257).abs() < 0.01);
        assert!((rows[0].max.unwrap() - 222.667).abs() < 0.01);
    }

    /// A unit outside the vocabulary must be held, not coerced — and the row
    /// must carry the declared unit, because HAE puts `units` on the metric
    /// and the point alone would not say what went wrong.
    #[tokio::test]
    async fn an_unconvertible_unit_quarantines_with_what_a_fix_needs() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "weight_body_mass", "units": "furlong",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 234.0 }] }
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
                .is_empty(),
            "nothing storable, so nothing stored"
        );
        assert_eq!(report.refused.len(), 1);
        assert_eq!(
            report.refused[0].reason,
            QuarantineReason::UnconvertibleUnit
        );
        assert_eq!(report.refused[0].kinds, vec![MetricKind::Weight]);
        assert!(!report.refused[0].stored);

        let rows = quarantined_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_name, "weight_body_mass");
        assert_eq!(rows[0].raw_point["qty"], 234.0, "the point, verbatim");
        assert_eq!(rows[0].raw_point["_import"]["reason"], "unconvertible-unit");
        assert_eq!(
            rows[0].raw_point["_import"]["units"], "furlong",
            "units live on the metric, so the point alone could not say"
        );
        assert_eq!(rows[0].raw_point["_import"]["kinds"][0], "weight");
    }

    /// No unit means no way to know what the number means. HAE's wire format
    /// always carries one, so assuming canonical would be guessing at exactly
    /// the moment the producer changed shape.
    #[tokio::test]
    async fn a_metric_with_no_declared_units_quarantines() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "weight_body_mass",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 234.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        assert_eq!(report.ingested, 0);
        assert_eq!(report.refused[0].reason, QuarantineReason::MissingUnit);
        assert_eq!(
            quarantined_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// Sleep's declared unit applies to every stage field in the point, and
    /// was previously ignored outright.
    #[tokio::test]
    async fn sleep_stage_hours_are_converted_from_the_declared_unit() {
        let db = test_db().await;
        ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "sleep_analysis", "units": "min",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700",
                             "totalSleep": 366.0, "deep": 78.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        let total = rows
            .iter()
            .find(|r| r.kind == MetricKind::SleepTotal)
            .unwrap();
        assert!(
            (total.value - 6.1).abs() < 1e-9,
            "366 minutes is 6.1 hours, got {}",
            total.value
        );
    }

    /// ADR-0002's posture, at point granularity: one metric being unusable
    /// must not cost the push everything else it carried.
    #[tokio::test]
    async fn one_bad_point_does_not_cost_the_days_other_metrics() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "weight_body_mass", "units": "furlong",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 234.0 }] },
                { "name": "step_count", "units": "count",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 8400.0 }] },
                { "name": "heart_rate", "units": "count/min",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700",
                             "Avg": 61.0, "Min": 48.0, "Max": 130.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        assert_eq!(report.ingested, 2, "the two good metrics still land");
        assert_eq!(report.refused.len(), 1);
        let kinds: Vec<_> = daily_metric::Entity::find()
            .all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.kind)
            .collect();
        assert!(kinds.contains(&MetricKind::Steps));
        assert!(kinds.contains(&MetricKind::HeartRate));
        assert!(!kinds.contains(&MetricKind::Weight));
    }

    /// A resolved complaint must not outlive its cause: upsert never deletes,
    /// so without the sweep every recovery would leave a row behind claiming a
    /// problem that no longer exists.
    #[tokio::test]
    async fn a_clean_re_push_retires_the_quarantine_row_it_replaces() {
        let db = test_db().await;
        let one = |units: &str| {
            payload(serde_json::json!([
                { "name": "weight_body_mass", "units": units,
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 234.0 }] }
            ]))
        };
        ingest_hae(&db, one("furlong")).await.expect("first");
        assert_eq!(
            quarantined_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .len(),
            1
        );

        let report = ingest_hae(&db, one("lb")).await.expect("second");
        assert_eq!(
            report.quarantine_retired, 1,
            "a run that deletes a standing complaint must say that it did"
        );
        assert!(
            quarantined_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .is_empty(),
            "the day now stores cleanly, so the complaint is retired"
        );
        assert_eq!(
            daily_metric::Entity::find().all(&db).await.unwrap().len(),
            1
        );
    }

    /// HAE's percent convention is unverified (healthie-t58), so the live path
    /// must not invent one: whatever arrives is what is stored. This is the
    /// test that flips on the day the question is answered.
    #[tokio::test]
    async fn percent_kinds_keep_the_scale_they_arrive_with() {
        let db = test_db().await;
        ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "body_fat_percentage", "units": "%",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 30.3 }] }
            ])),
        )
        .await
        .expect("ingest");
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert!(
            (rows[0].value - 30.3).abs() < f64::EPSILON,
            "HAE is assumed to send 0-100 already; nothing scales it"
        );
    }

    /// healthie-55h, live side. The two halves of one rule: a value we cannot
    /// trust costs the point, a *bound* we cannot trust costs only that column.
    ///
    /// The row is kept because the day's average is sound and independently
    /// measured — discarding it to punish a sensor glitch in the spread would
    /// throw away good data. The point is still quarantined verbatim, so the
    /// dropped number is recoverable: the two tables are independent.
    #[tokio::test]
    async fn an_impossible_bound_costs_the_column_but_not_the_row() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "blood_oxygen_saturation", "units": "%",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700",
                             "Avg": 96.4, "Min": 0.0, "Max": 99.0 }] }
            ])),
        )
        .await
        .expect("ingest");

        assert_eq!(report.ingested, 1, "the row still lands");
        assert_eq!(report.bounds_cleared, 1);
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert!((rows[0].value - 96.4).abs() < f64::EPSILON);
        assert_eq!(rows[0].min, None, "0% saturation is not a measurement");
        assert!(
            (rows[0].max.unwrap() - 99.0).abs() < f64::EPSILON,
            "the trustworthy bound survives"
        );

        assert_eq!(report.refused.len(), 1);
        assert!(report.refused[0].stored, "a row landed despite the refusal");
        assert_eq!(report.refused[0].reason, QuarantineReason::ImplausibleValue);
        assert_eq!(
            quarantined_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .len(),
            1,
            "the discarded number is still recoverable"
        );
    }

    /// Two points on one `(name, date)` with different fates. `quarantined_metric`
    /// is keyed per day while refusals are per point, so the reported outcome
    /// must describe the database rather than whichever point came first in the
    /// array.
    #[tokio::test]
    async fn a_day_that_both_refused_and_stored_reports_that_a_row_landed() {
        let db = test_db().await;
        let refused_first = serde_json::json!([
            { "name": "blood_oxygen_saturation", "units": "%", "data": [
                { "date": "2026-07-28 00:00:00 -0700", "qty": 0.0 },
                { "date": "2026-07-28 00:00:00 -0700", "Avg": 96.4, "Min": 0.0, "Max": 99.0 }
            ] }
        ]);
        let mut reversed = refused_first.clone();
        reversed[0]["data"].as_array_mut().unwrap().reverse();

        for (order, metrics) in [("refused first", refused_first), ("stored first", reversed)] {
            let db = if order == "refused first" {
                &db
            } else {
                &test_db().await
            };
            let report = ingest_hae(db, payload(metrics)).await.expect("ingest");
            let rows = daily_metric::Entity::find().all(db).await.unwrap();
            assert_eq!(rows.len(), 1, "{order}: the good aggregate still lands");
            assert!((rows[0].value - 96.4).abs() < f64::EPSILON, "{order}");
            assert_eq!(
                rows[0].min, None,
                "{order}: the impossible floor is dropped"
            );
            assert_eq!(report.refused.len(), 1, "{order}: one day, one complaint");
            assert!(
                report.refused[0].stored,
                "{order}: a row IS in the table, so the report must not say otherwise"
            );
        }
    }

    /// An impossible `value` has nothing storable in it, so no row lands.
    #[tokio::test]
    async fn an_impossible_value_refuses_the_whole_point() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "body_fat_percentage", "units": "%",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "qty": 0.0 }] }
            ])),
        )
        .await
        .expect("ingest");
        assert_eq!(report.ingested, 0);
        assert_eq!(report.bounds_cleared, 0);
        assert!(!report.refused[0].stored);
        assert!(
            daily_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// healthie-55h's confirmed case, at stage granularity: 2023-12-29's sleep
    /// app ran for a day and a half. The impossible total goes; the ordinary
    /// stages recorded beside it stay, and the quarantine row names exactly
    /// which kinds were refused.
    #[tokio::test]
    async fn an_impossible_sleep_stage_refuses_only_that_stage() {
        let db = test_db().await;
        let report = ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "sleep_analysis", "units": "hr",
                  "data": [{ "date": "2023-12-29 00:00:00 -0700",
                             "totalSleep": 49.877, "inBed": 52.953,
                             "deep": 1.36, "rem": 1.34, "core": 3.4 }] }
            ])),
        )
        .await
        .expect("ingest");

        assert_eq!(report.ingested, 3, "deep, rem and core are ordinary nights");
        let kinds: Vec<_> = daily_metric::Entity::find()
            .all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.kind)
            .collect();
        assert!(kinds.contains(&MetricKind::SleepDeep));
        assert!(kinds.contains(&MetricKind::SleepRem));
        assert!(kinds.contains(&MetricKind::SleepCore));
        assert!(!kinds.contains(&MetricKind::SleepTotal));
        assert!(!kinds.contains(&MetricKind::TimeInBed));

        assert_eq!(report.refused.len(), 1, "one point, so one complaint");
        assert_eq!(
            report.refused[0].kinds,
            vec![MetricKind::SleepTotal, MetricKind::TimeInBed]
        );
        let rows = quarantined_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows[0].raw_point["_import"]["reason"], "implausible-value");
        assert_eq!(rows[0].raw_point["_import"]["kinds"][0], "sleep-total");
        assert_eq!(rows[0].raw_point["_import"]["kinds"][1], "time-in-bed");
    }

    #[tokio::test]
    async fn aggregate_without_min_max_stores_avg_only() {
        let db = test_db().await;
        ingest_hae(
            &db,
            payload(serde_json::json!([
                { "name": "heart_rate", "units": "count/min",
                  "data": [{ "date": "2026-07-28 00:00:00 -0700", "Avg": 61.0 }] }
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
