//! One-time backfill of Apple Health's `export.xml` into the curated
//! `daily_metric` store (ADR-0005, ADR-0006).
//!
//! This is the sibling of the live HAE ingest in [`crate::services::metrics`],
//! not its replacement, and the asymmetry between them is the whole problem:
//! HAE posts **pre-aggregated** daily points, while `export.xml` is raw
//! per-reading records. The backfill therefore reconstructs the daily rollup
//! HAE gets for free — see [`accumulate`] for the per-metric policy.
//!
//! # Shape of a run
//!
//! [`parse_export_xml`] is synchronous, touches no database, and does the
//! expensive work; [`persist_import`] takes its opaque [`ParsedExport`] and
//! writes. Callers on an async runtime should run the first on a blocking
//! thread — a multi-GB parse takes minutes and would otherwise stall a worker.
//!
//! # Properties worth knowing before reading the data
//!
//! - **Sum-kind values are a lower bound.** Apple's export retains every
//!   device's account of the same day, so `Steps` and friends take the largest
//!   single source's total rather than summing across sources. Two devices that
//!   each captured a disjoint half-day therefore yield only the larger half.
//!   The import report prints how many days had several sources and how much
//!   was discarded.
//! - **`SleepTotal` can be less than the sum of its stages.** It is the union
//!   of asleep intervals, so stages recorded by different sources that overlap
//!   in time are counted once in the total but separately per stage.
//! - **Quarantine is one row per name, not per `(name, date)`.** This narrows
//!   ADR-0005 §4 for this path only; see ADR-0006.
//! - **Writes are last-write-wins on `(kind, date)`**, so a backfill overwrites
//!   rows a live HAE push already landed. The report compares against those
//!   rows *before* overwriting them, because that comparison is the only
//!   available check on whether the two paths agree.

pub(crate) mod accumulate;
pub(crate) mod mapping;
pub(crate) mod parse;
pub(crate) mod units;

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use chrono::NaiveDate;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, TransactionTrait};

use self::{
    accumulate::{Accumulator, PendingRow},
    mapping::{is_recognized_hk_name, map_sleep_stage},
    parse::ImportStats,
};
use crate::{
    clock,
    entities::{daily_metric, daily_metric::MetricKind, quarantined_metric},
    error::{DomainError, DomainResult},
    services::metrics::{upsert_metric, upsert_quarantine},
};

/// Prefix identifying an `export.xml`-vocabulary quarantine row.
///
/// `quarantined_metric` holds two vocabularies with no column to tell them
/// apart: HAE's `snake_case` names and Apple's `HK…` identifiers. Until
/// healthie-1ru adds a real discriminator, this prefix is the interim one — it
/// is sufficient here because it can never match an HAE name, so the backfill's
/// cleanup can never touch a row the live path wrote.
const HK_PREFIX: &str = "HK";

/// The parsed contents of an `export.xml`, ready to persist.
///
/// Deliberately opaque: it carries the accumulator and running counts, whose
/// shapes are implementation detail. Obtain it from [`parse_export_xml`] and
/// hand it to [`persist_import`].
pub struct ParsedExport {
    accumulator: Accumulator,
    stats: ImportStats,
}

/// An uncurated name found in the export, with how many records carried it.
#[derive(Debug, Clone)]
pub struct QuarantinedName {
    pub raw_name: String,
    pub records_seen: u64,
}

/// A `(type, unit)` pair no conversion covers.
#[derive(Debug, Clone)]
pub struct UnconvertibleUnit {
    pub raw_name: String,
    pub unit: String,
    pub records: u64,
}

/// Per-kind summary of what was written, including the observed value span.
///
/// The span is a deliberate tripwire: `export.xml`'s percent-typed quantities
/// are undocumented as to scale, so the run prints what it wrote rather than
/// guessing. A spo2 span of `0.91 .. 0.99` instead of `91 .. 99` is obvious.
#[derive(Debug, Clone)]
pub struct KindReport {
    pub kind: MetricKind,
    pub unit: &'static str,
    pub days: usize,
    pub value_min: f64,
    pub value_max: f64,
    /// Rows that already existed and were overwritten, and by how much they
    /// differed. `None` when this kind overwrote nothing.
    pub overlap: Option<Overlap>,
}

/// How far a reconstructed value diverged from a row that was already there.
#[derive(Debug, Clone, Copy)]
pub struct Overlap {
    pub days: usize,
    pub mean_abs_diff: f64,
    pub max_abs_diff: f64,
    pub max_diff_date: NaiveDate,
}

/// How much multi-source de-duplication discarded for one `Sum` kind.
#[derive(Debug, Clone, Copy)]
pub struct SumSourceReport {
    pub kind: MetricKind,
    pub days_multi_source: usize,
    pub mean_ratio: f64,
    pub worst_ratio: f64,
}

/// Whether backfilled sleep lands on the same calendar day as sleep already in
/// the store.
///
/// A day-shifted sleep row is invisible to every other check — its value span
/// looks perfectly normal — so this compares each reconstructed `SleepTotal`
/// against existing rows one day either side. It can only say anything where
/// stored rows and imported nights overlap; otherwise it reports
/// [`Self::NoComparableRows`] rather than a table of zeros that would read as
/// agreement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SleepDayShift {
    /// Nothing to compare against: either no `SleepTotal` rows are stored at
    /// all, or none fall within a day of anything this import produced (an old
    /// export landing beside newer history). Both mean the boundary was simply
    /// not checked by this run.
    NoComparableRows,
    Compared {
        compared_days: usize,
        /// Mean |ours(D) − existing(D−1)|.
        prev_day: Option<f64>,
        /// Mean |ours(D) − existing(D)|.
        same_day: Option<f64>,
        /// Mean |ours(D) − existing(D+1)|.
        next_day: Option<f64>,
    },
}

/// A neighbouring day must fit at least this much better, proportionally,
/// before we call the boundary wrong.
const MATERIAL_SHIFT_RATIO: f64 = 0.5;

/// …and by at least this many hours in absolute terms. Together these stop a
/// tie or ordinary noise from raising an alarm that tells the operator to go
/// change a constant.
const MATERIAL_SHIFT_HOURS: f64 = 0.25;

/// What the day-shift comparison concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepDayVerdict {
    /// Nothing to compare against — the boundary was not checked by this run.
    Unverified,
    /// Same-day fits, or no neighbour fits materially better.
    Agrees,
    /// A neighbouring day fits decisively better; `offset` is which one.
    Mismatch { offset: i8 },
}

impl SleepDayShift {
    /// The offset that fits best, or `None` when there is nothing to compare.
    ///
    /// Ties resolve to `0`: equal fits carry no evidence of a shift, and the
    /// same day is the null hypothesis.
    #[must_use]
    pub fn best_offset(&self) -> Option<i8> {
        let Self::Compared {
            prev_day,
            same_day,
            next_day,
            ..
        } = self
        else {
            return None;
        };
        // Offset 0 is listed first so `min_by`, which keeps the first minimum,
        // returns it whenever the fits are equal.
        [(0i8, same_day), (-1, prev_day), (1, next_day)]
            .into_iter()
            .filter_map(|(offset, diff)| diff.map(|d| (offset, d)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(offset, _)| offset)
    }

    /// Whether the sleep-day boundary agrees with the rows already stored.
    ///
    /// Deliberately conservative. This warning tells the operator to change a
    /// constant and re-import a decade of history, so it fires only when a
    /// neighbouring day fits both proportionally and absolutely better — never
    /// on a tie, and never on noise.
    #[must_use]
    pub fn verdict(&self) -> SleepDayVerdict {
        let Self::Compared {
            prev_day,
            same_day,
            next_day,
            ..
        } = self
        else {
            return SleepDayVerdict::Unverified;
        };
        let Some(offset) = self.best_offset() else {
            return SleepDayVerdict::Unverified;
        };
        if offset == 0 {
            return SleepDayVerdict::Agrees;
        }
        // A neighbour won, but only counts if same-day is measurably worse.
        let (Some(same), Some(best)) = (*same_day, if offset < 0 { *prev_day } else { *next_day })
        else {
            // No same-day rows at all to compare against, so the neighbour
            // "winning" is an artefact of missing data, not evidence.
            return SleepDayVerdict::Agrees;
        };
        if best < same * MATERIAL_SHIFT_RATIO && (same - best) >= MATERIAL_SHIFT_HOURS {
            SleepDayVerdict::Mismatch { offset }
        } else {
            SleepDayVerdict::Agrees
        }
    }
}

/// Everything one import did, for the operator to read.
#[derive(Debug, Clone)]
pub struct ImportReport {
    pub records_read: u64,
    pub records_curated: u64,
    pub records_excluded: u64,
    pub records_skipped: u64,
    pub rows_written: usize,
    pub rows_overwritten: usize,
    pub stale_quarantine_cleared: usize,
    pub date_range: Option<(NaiveDate, NaiveDate)>,
    pub quarantined: Vec<QuarantinedName>,
    pub unconvertible: Vec<UnconvertibleUnit>,
    pub per_kind: Vec<KindReport>,
    pub sum_sources: Vec<SumSourceReport>,
    pub sleep_day_shift: SleepDayShift,
}

/// Parse an `export.xml` from disk. Synchronous and database-free.
///
/// # Errors
/// Returns [`DomainError::Internal`] if the file cannot be opened, or if the
/// XML is malformed — the latter carrying the offending byte offset, which is
/// the only practical way to locate a fault in a multi-gigabyte file.
/// Individual unusable records are counted, not fatal.
pub fn parse_export_xml(path: &Path) -> DomainResult<ParsedExport> {
    let file =
        File::open(path).map_err(|e| DomainError::Internal(format!("{}: {e}", path.display())))?;
    parse_export_reader(BufReader::new(file))
}

/// Parse an `export.xml` from any reader.
///
/// # Errors
/// As [`parse_export_xml`].
pub fn parse_export_reader<R: BufRead>(reader: R) -> DomainResult<ParsedExport> {
    let mut accumulator = Accumulator::default();
    let mut stats = ImportStats::default();
    parse::parse_into(reader, &mut accumulator, &mut stats)?;
    Ok(ParsedExport { accumulator, stats })
}

/// Persist a parsed export: curated rows into `daily_metric`, uncurated names
/// into `quarantined_metric`, all in one transaction.
///
/// Idempotent — `(kind, date)` upserts last-write-wins, so re-running after
/// fixing a mapping re-lands cleanly instead of duplicating.
///
/// # Errors
/// Returns [`DomainError::Db`] on database failure.
pub async fn persist_import<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    parsed: ParsedExport,
) -> DomainResult<ImportReport> {
    let ParsedExport { accumulator, stats } = parsed;
    let resolved = accumulator.resolve();
    let now = clock::now();
    let txn = db.begin().await?;

    // Compare against what is already stored BEFORE overwriting it — these rows
    // are about to be replaced, and they are the only cross-check available on
    // whether the reconstruction agrees with the live path.
    let existing_sleep = load_sleep_totals(&txn).await?;
    let sleep_day_shift = sleep_day_shift(&resolved.rows, &existing_sleep);

    let stale_quarantine_cleared = clear_promoted_quarantine(&txn).await?;

    let mut overlaps: BTreeMap<MetricKind, OverlapAcc> = BTreeMap::new();
    let mut rows_overwritten = 0usize;
    let (mut min_date, mut max_date) = (None::<NaiveDate>, None::<NaiveDate>);

    for row in &resolved.rows {
        let prior = upsert_metric(
            &txn,
            row.kind,
            row.date,
            row.value,
            row.min,
            row.max,
            row.source.clone(),
            now,
        )
        .await?;
        if let Some(prior) = prior {
            rows_overwritten += 1;
            overlaps
                .entry(row.kind)
                .or_default()
                .fold(row.date, (row.value - prior.value).abs());
        }
        min_date = Some(min_date.map_or(row.date, |d: NaiveDate| d.min(row.date)));
        max_date = Some(max_date.map_or(row.date, |d: NaiveDate| d.max(row.date)));
    }

    for (raw_name, sample) in &stats.quarantined {
        let mut point = sample.point.clone();
        if let Some(meta) = point
            .get_mut("_import")
            .and_then(serde_json::Value::as_object_mut)
        {
            meta.insert("records_seen".to_owned(), sample.records_seen.into());
        }
        // `upsert_quarantine` keys on `(raw_name, date)`, and the retained date
        // is whichever record this run happened to see first. Importing a
        // second export that reaches further back would otherwise land a
        // *second* row for the same name, quietly breaking the one-row-per-name
        // invariant this path promises. Drop the other dates first.
        drop_other_dates(&txn, raw_name, sample.date).await?;
        upsert_quarantine(&txn, raw_name, sample.date, &point, now).await?;
    }

    txn.commit().await?;

    Ok(ImportReport {
        records_read: stats.records_read,
        records_curated: stats.records_curated,
        records_excluded: stats.records_excluded,
        records_skipped: stats.records_skipped,
        rows_written: resolved.rows.len(),
        rows_overwritten,
        stale_quarantine_cleared,
        date_range: min_date.zip(max_date),
        quarantined: stats
            .quarantined
            .iter()
            .map(|(raw_name, sample)| QuarantinedName {
                raw_name: raw_name.clone(),
                records_seen: sample.records_seen,
            })
            .collect(),
        unconvertible: stats
            .unconvertible
            .iter()
            .map(|((raw_name, unit), records)| UnconvertibleUnit {
                raw_name: raw_name.clone(),
                unit: unit.clone(),
                records: *records,
            })
            .collect(),
        per_kind: resolved
            .per_kind
            .iter()
            .map(|(kind, summary)| KindReport {
                kind: *kind,
                unit: kind.unit(),
                days: summary.days,
                value_min: summary.value_min,
                value_max: summary.value_max,
                overlap: overlaps.get(kind).map(OverlapAcc::finish),
            })
            .collect(),
        sum_sources: resolved
            .sum_sources
            .iter()
            .map(|(kind, s)| SumSourceReport {
                kind: *kind,
                days_multi_source: s.days_multi_source,
                mean_ratio: s.mean_ratio,
                worst_ratio: s.worst_ratio,
            })
            .collect(),
        sleep_day_shift,
    })
}

#[derive(Default)]
struct OverlapAcc {
    days: usize,
    total: f64,
    max: f64,
    max_date: Option<NaiveDate>,
}

impl OverlapAcc {
    fn fold(&mut self, date: NaiveDate, diff: f64) {
        self.days += 1;
        self.total += diff;
        if self.max_date.is_none() || diff > self.max {
            self.max = diff;
            self.max_date = Some(date);
        }
    }

    fn finish(&self) -> Overlap {
        // A day tally; f64 is exact well past this.
        #[allow(clippy::cast_precision_loss)]
        let days = self.days as f64;
        Overlap {
            days: self.days,
            mean_abs_diff: self.total / days,
            max_abs_diff: self.max,
            max_diff_date: self.max_date.unwrap_or_default(),
        }
    }
}

async fn load_sleep_totals<C: ConnectionTrait>(db: &C) -> DomainResult<BTreeMap<NaiveDate, f64>> {
    Ok(daily_metric::Entity::find()
        .filter(daily_metric::Column::Kind.eq(MetricKind::SleepTotal))
        .all(db)
        .await?
        .into_iter()
        .map(|row| (row.date, row.value))
        .collect())
}

/// Reasons that describe an unrecognized *name*, and so stop applying the
/// moment that name joins the vocabulary.
///
/// Deliberately not every reason: a curated metric can also be quarantined
/// because one record carried an unconvertible or missing unit, and those rows
/// describe a live data problem that promoting the name does nothing about.
/// Sweeping them would erase a standing complaint just because the metric
/// happens to be curated.
const NAME_BASED_QUARANTINE_REASONS: &[&str] = &["unknown-type", "unknown-sleep-stage"];

/// Delete quarantine rows for `export.xml` names this build now handles.
///
/// Upsert never deletes, so without this a name promoted into the curated
/// vocabulary would keep an old quarantine row advertising it as unhandled long
/// after a re-run had imported it properly.
async fn clear_promoted_quarantine<C: ConnectionTrait>(db: &C) -> DomainResult<usize> {
    let stale: Vec<i32> = quarantined_metric::Entity::find()
        .filter(quarantined_metric::Column::RawName.starts_with(HK_PREFIX))
        .all(db)
        .await?
        .into_iter()
        .filter(|row| {
            let name_now_handled =
                is_recognized_hk_name(&row.raw_name) || map_sleep_stage(&row.raw_name).is_some();
            name_now_handled && quarantined_for_its_name(&row.raw_point)
        })
        .map(|row| row.id)
        .collect();
    if stale.is_empty() {
        return Ok(0);
    }
    let cleared = stale.len();
    quarantined_metric::Entity::delete_many()
        .filter(quarantined_metric::Column::Id.is_in(stale))
        .exec(db)
        .await?;
    Ok(cleared)
}

/// Remove any quarantine row for `raw_name` dated other than `keep`.
///
/// Scoped to the `HK` vocabulary so it can never reach a row the live HAE path
/// wrote, even in the impossible case of a name collision.
async fn drop_other_dates<C: ConnectionTrait>(
    db: &C,
    raw_name: &str,
    keep: NaiveDate,
) -> DomainResult<()> {
    quarantined_metric::Entity::delete_many()
        .filter(quarantined_metric::Column::RawName.eq(raw_name))
        .filter(quarantined_metric::Column::RawName.starts_with(HK_PREFIX))
        .filter(quarantined_metric::Column::Date.ne(keep))
        .exec(db)
        .await?;
    Ok(())
}

/// Whether a quarantine row exists because its *name* was unrecognized.
///
/// A row with no recorded reason predates the reason field and can only have
/// come from an unrecognized name, so it is sweepable.
fn quarantined_for_its_name(raw_point: &serde_json::Value) -> bool {
    raw_point
        .get("_import")
        .and_then(|meta| meta.get("reason"))
        .and_then(serde_json::Value::as_str)
        .is_none_or(|reason| NAME_BASED_QUARANTINE_REASONS.contains(&reason))
}

/// Mean absolute difference between reconstructed `SleepTotal` values and
/// existing rows at day offsets −1, 0 and +1.
fn sleep_day_shift(rows: &[PendingRow], existing: &BTreeMap<NaiveDate, f64>) -> SleepDayShift {
    if existing.is_empty() {
        return SleepDayShift::NoComparableRows;
    }
    let mut sums = [(0.0f64, 0usize); 3];
    let mut compared_days = 0usize;
    for row in rows.iter().filter(|r| r.kind == MetricKind::SleepTotal) {
        let mut matched = false;
        for (slot, offset) in [(0usize, -1i64), (1, 0), (2, 1)] {
            let Some(date) = row.date.checked_add_signed(chrono::Duration::days(offset)) else {
                continue;
            };
            if let Some(prior) = existing.get(&date) {
                sums[slot].0 += (row.value - prior).abs();
                sums[slot].1 += 1;
                matched = true;
            }
        }
        if matched {
            compared_days += 1;
        }
    }
    if compared_days == 0 {
        return SleepDayShift::NoComparableRows;
    }
    let mean = |(total, count): (f64, usize)| {
        // A day tally; f64 is exact well past this.
        #[allow(clippy::cast_precision_loss)]
        let n = count as f64;
        (count > 0).then(|| total / n)
    };
    SleepDayShift::Compared {
        compared_days,
        prev_day: mean(sums[0]),
        same_day: mean(sums[1]),
        next_day: mean(sums[2]),
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, EntityTrait};

    use super::{
        ImportReport, SleepDayShift, SleepDayVerdict, parse_export_reader, persist_import,
    };
    use crate::{
        entities::{daily_metric, daily_metric::MetricKind, quarantined_metric},
        services::metrics::{HaePayload, ingest_hae},
        test_support::{date, datetime, test_db},
    };

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <HealthData locale="en_US">
      <ExportDate value="2026-07-31 09:00:00 -0700"/>
      <Record type="HKQuantityTypeIdentifierBodyMass" sourceName="Withings" unit="kg"
              startDate="2026-07-28 06:14:02 -0700" endDate="2026-07-28 06:14:02 -0700" value="100"/>
      <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Watch" unit="count/min"
              startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="50"/>
      <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Watch" unit="count/min"
              startDate="2026-07-28 20:00:00 -0700" endDate="2026-07-28 20:00:00 -0700" value="90"/>
      <Record type="HKQuantityTypeIdentifierBasalEnergyBurned" sourceName="Watch" unit="Cal"
              startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="1800"/>
      <Record type="HKQuantityTypeIdentifierDietaryWater" sourceName="App" unit="mL"
              startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="240"/>
      <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
              value="HKCategoryValueSleepAnalysisAsleepCore"
              startDate="2026-07-27 23:00:00 -0700" endDate="2026-07-28 05:00:00 -0700"/>
    </HealthData>"#;

    async fn import<C: ConnectionTrait + sea_orm::TransactionTrait>(
        db: &C,
        xml: &str,
    ) -> ImportReport {
        let parsed = parse_export_reader(xml.as_bytes()).expect("parse");
        persist_import(db, parsed).await.expect("persist")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[tokio::test]
    async fn import_writes_rows_quarantine_and_report() {
        let db = test_db().await;
        let report = import(&db, FIXTURE).await;

        // weight + heart-rate + sleep-core + sleep-total = 4 rows.
        assert_eq!(report.rows_written, 4);
        assert_eq!(report.rows_overwritten, 0);
        assert_eq!(report.records_excluded, 1, "basal energy is declined");
        assert_eq!(
            report.date_range,
            Some((date("2026-07-28"), date("2026-07-28")))
        );

        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 4);
        let weight = rows.iter().find(|r| r.kind == MetricKind::Weight).unwrap();
        assert!(
            close(weight.value, 220.462_262_184_877_6),
            "kg converted to lb"
        );
        let hr = rows
            .iter()
            .find(|r| r.kind == MetricKind::HeartRate)
            .unwrap();
        assert_eq!((hr.min, hr.max), (Some(50.0), Some(90.0)));

        let quarantined = quarantined_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(
            quarantined[0].raw_name,
            "HKQuantityTypeIdentifierDietaryWater"
        );
        assert_eq!(quarantined[0].raw_point["_import"]["records_seen"], 1);
    }

    /// The per-kind value span is the only guard against a scaling mistake in
    /// an undocumented unit, so it has to actually be populated.
    #[tokio::test]
    async fn report_carries_per_kind_value_spans() {
        let db = test_db().await;
        let report = import(&db, FIXTURE).await;

        let hr = report
            .per_kind
            .iter()
            .find(|k| k.kind == MetricKind::HeartRate)
            .expect("heart rate summarized");
        assert_eq!(hr.days, 1);
        assert_eq!(hr.unit, "count/min");
        assert!(close(hr.value_min, 70.0) && close(hr.value_max, 70.0));

        let weight = report
            .per_kind
            .iter()
            .find(|k| k.kind == MetricKind::Weight)
            .expect("weight summarized");
        assert!(
            close(weight.value_min, 220.462_262_184_877_6),
            "the span must show converted values, not raw kg"
        );
        assert!(report.per_kind.iter().all(|k| k.overlap.is_none()));
    }

    #[tokio::test]
    async fn import_is_idempotent() {
        let db = test_db().await;
        let first = import(&db, FIXTURE).await;
        let second = import(&db, FIXTURE).await;

        assert_eq!(first.rows_written, second.rows_written);
        assert_eq!(
            second.rows_overwritten, second.rows_written,
            "all re-landed"
        );
        let rows = daily_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 4, "re-running must upsert, not duplicate");
        let quarantined = quarantined_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(quarantined.len(), 1);
    }

    /// Last-write-wins means the backfill overwrites live HAE rows. It must
    /// report how far it diverged from them *before* replacing them — that
    /// comparison is the only cross-check the two paths ever get.
    #[tokio::test]
    async fn overlap_with_existing_rows_is_reported_before_overwriting() {
        let db = test_db().await;
        let payload: HaePayload = serde_json::from_value(serde_json::json!({
            "data": { "metrics": [
                { "name": "weight_body_mass", "units": "lb",
                  "data": [{ "date": "2026-07-28 06:00:00 -0700", "qty": 210.0 }] }
            ]}
        }))
        .expect("payload");
        ingest_hae(&db, payload).await.expect("seed live row");

        let report = import(&db, FIXTURE).await;

        assert_eq!(report.rows_overwritten, 1);
        let overlap = report
            .per_kind
            .iter()
            .find(|k| k.kind == MetricKind::Weight)
            .and_then(|k| k.overlap)
            .expect("weight overlapped an existing row");
        assert_eq!(overlap.days, 1);
        assert!(
            close(overlap.max_abs_diff, 220.462_262_184_877_6 - 210.0),
            "the divergence from the live row must be quantified, got {}",
            overlap.max_abs_diff
        );
        assert_eq!(overlap.max_diff_date, date("2026-07-28"));

        // And the import still won.
        let row = daily_metric::Entity::find()
            .all(&db)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.kind == MetricKind::Weight)
            .unwrap();
        assert!(close(row.value, 220.462_262_184_877_6));
    }

    /// A fresh database has nothing to compare against; saying so is important,
    /// because a table of zeros would read as "the boundary checks out".
    #[tokio::test]
    async fn sleep_day_shift_is_inconclusive_without_prior_rows() {
        let db = test_db().await;
        let report = import(&db, FIXTURE).await;
        assert_eq!(report.sleep_day_shift, SleepDayShift::NoComparableRows);
        assert_eq!(report.sleep_day_shift.best_offset(), None);
    }

    /// If the sleep-day boundary disagreed with whatever wrote the existing
    /// rows, every backfilled night would sit one day off — invisible to every
    /// other check, since a shifted row's value looks perfectly normal.
    #[tokio::test]
    async fn sleep_day_shift_detects_an_offset_night() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        // Existing rows carrying our night's 6.0 hours one day EARLIER, plus
        // unrelated neighbours so the comparison has something to reject.
        for (day, hours) in [
            ("2026-07-27", 6.0),
            ("2026-07-28", 1.0),
            ("2026-07-29", 1.0),
        ] {
            daily_metric::ActiveModel {
                kind: Set(MetricKind::SleepTotal),
                date: Set(date(day)),
                value: Set(hours),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(&db)
            .await
            .expect("seed");
        }

        let report = import(
            &db,
            r#"<HealthData>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                      value="HKCategoryValueSleepAnalysisAsleepCore"
                      startDate="2026-07-27 23:00:00 -0700" endDate="2026-07-28 05:00:00 -0700"/>
            </HealthData>"#,
        )
        .await;

        let SleepDayShift::Compared { compared_days, .. } = report.sleep_day_shift else {
            panic!("expected a comparison, got {:?}", report.sleep_day_shift);
        };
        assert_eq!(compared_days, 1);
        assert_eq!(
            report.sleep_day_shift.best_offset(),
            Some(-1),
            "our 2026-07-28 row matches the existing 2026-07-27 row: {:?}",
            report.sleep_day_shift
        );
        assert_eq!(
            report.sleep_day_shift.verdict(),
            SleepDayVerdict::Mismatch { offset: -1 }
        );
    }

    /// Re-importing unchanged data fits every offset equally well. That is the
    /// common case on a second run, and it is not evidence of a shift.
    #[tokio::test]
    async fn identical_reimport_does_not_report_a_boundary_mismatch() {
        let db = test_db().await;
        import(&db, FIXTURE).await;
        let second = import(&db, FIXTURE).await;

        assert_eq!(
            second.sleep_day_shift.best_offset(),
            Some(0),
            "ties must resolve to same-day: {:?}",
            second.sleep_day_shift
        );
        assert_eq!(second.sleep_day_shift.verdict(), SleepDayVerdict::Agrees);
    }

    #[test]
    fn a_marginal_neighbour_is_not_a_mismatch() {
        // Better, but neither proportionally nor absolutely decisive.
        let shift = SleepDayShift::Compared {
            compared_days: 100,
            prev_day: Some(0.30),
            same_day: Some(0.34),
            next_day: None,
        };
        assert_eq!(shift.best_offset(), Some(-1));
        assert_eq!(shift.verdict(), SleepDayVerdict::Agrees);

        // Decisively better on both counts.
        let shift = SleepDayShift::Compared {
            compared_days: 100,
            prev_day: Some(0.10),
            same_day: Some(4.80),
            next_day: None,
        };
        assert_eq!(shift.verdict(), SleepDayVerdict::Mismatch { offset: -1 });
    }

    /// Upsert never deletes, so a name promoted into the curated vocabulary
    /// would keep an old quarantine row claiming it is unhandled. The sweep is
    /// scoped by the `HK` prefix and must not touch HAE-vocabulary rows.
    #[tokio::test]
    async fn promoting_a_name_clears_its_stale_quarantine_row() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        for (raw_name, reason) in [
            // Curated today — a leftover from before it was promoted.
            ("HKQuantityTypeIdentifierBodyMass", "unknown-type"),
            // A sleep stage we now understand.
            (
                "HKCategoryValueSleepAnalysisAsleepREM",
                "unknown-sleep-stage",
            ),
            // Still uncurated: must survive.
            ("HKQuantityTypeIdentifierDietaryWater", "unknown-type"),
            // Curated name, but quarantined over a UNIT it could not convert.
            // Promoting the name did not fix that, so it must survive.
            ("HKQuantityTypeIdentifierStepCount", "unconvertible-unit"),
        ] {
            quarantined_metric::ActiveModel {
                raw_name: Set(raw_name.to_owned()),
                date: Set(date("2026-07-01")),
                raw_point: Set(serde_json::json!({ "_import": { "reason": reason } })),
                created_at: Set(now),
                ..Default::default()
            }
            .insert(&db)
            .await
            .expect("seed quarantine");
        }
        // HAE vocabulary: the backfill's sweep must never reach it.
        quarantined_metric::ActiveModel {
            raw_name: Set("some_future_hae_metric".to_owned()),
            date: Set(date("2026-07-01")),
            raw_point: Set(serde_json::json!({ "qty": 1.0 })),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("seed HAE quarantine");

        let report = import(&db, FIXTURE).await;
        assert_eq!(report.stale_quarantine_cleared, 2);

        let names: Vec<String> = quarantined_metric::Entity::find()
            .all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.raw_name)
            .collect();
        assert!(
            names.contains(&"some_future_hae_metric".to_owned()),
            "the HK-prefix scope must leave HAE rows alone"
        );
        assert!(names.contains(&"HKQuantityTypeIdentifierDietaryWater".to_owned()));
        assert!(
            names.contains(&"HKQuantityTypeIdentifierStepCount".to_owned()),
            "a unit problem is not resolved by the name being curated"
        );
        assert!(!names.contains(&"HKCategoryValueSleepAnalysisAsleepREM".to_owned()));
        // BodyMass was swept as a promoted name, then re-quarantined only if the
        // fixture still has a problem record for it — it does not.
        assert!(!names.contains(&"HKQuantityTypeIdentifierBodyMass".to_owned()));
    }

    /// One row per name has to hold ACROSS runs, not just within one. The
    /// retained date is whichever record a run saw first, so importing a second
    /// export reaching further back would otherwise land a second row for the
    /// same name under a different `(raw_name, date)` key.
    #[tokio::test]
    async fn a_second_export_does_not_add_a_row_for_the_same_name() {
        let db = test_db().await;
        let water = |day: &str| {
            format!(
                r#"<HealthData>
                  <Record type="HKQuantityTypeIdentifierDietaryWater" unit="mL"
                          startDate="{day} 08:00:00 -0700" endDate="{day} 08:00:00 -0700" value="240"/>
                </HealthData>"#
            )
        };

        import(&db, &water("2026-07-28")).await;
        // A second, older export: same uncurated name, earlier first sighting.
        import(&db, &water("2015-03-11")).await;

        let rows = quarantined_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(
            rows.len(),
            1,
            "one row per name must survive a differently-dated export, got {rows:?}"
        );
        assert_eq!(
            rows[0].date,
            date("2015-03-11"),
            "the latest run's sighting wins"
        );
    }

    /// The regression the smoke test caught: re-running must not churn a
    /// quarantine row that records a still-unresolved unit problem.
    #[tokio::test]
    async fn rerun_leaves_unit_quarantine_rows_untouched() {
        let db = test_db().await;
        let xml = r#"<HealthData>
          <Record type="HKQuantityTypeIdentifierBodyMass" unit="mmHg"
                  startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="118"/>
        </HealthData>"#;

        let first = import(&db, xml).await;
        assert_eq!(first.stale_quarantine_cleared, 0);
        let second = import(&db, xml).await;
        assert_eq!(
            second.stale_quarantine_cleared, 0,
            "a curated name quarantined over its unit must not be swept as 'promoted'"
        );
        let rows = quarantined_metric::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_point["_import"]["reason"], "unconvertible-unit");
    }
}
