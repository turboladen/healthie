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
//!   available check on whether the two paths agree — and only on the first
//!   run, since afterwards those rows are this importer's own output.
//! - **Re-running is idempotent for value changes, not for key changes.** A
//!   corrected unit or aggregation rewrites the same `(kind, date)` rows. A
//!   corrected *sleep-day boundary* or metric mapping moves rows to different
//!   dates or kinds, and nothing deletes what sat at the old ones — see
//!   [`ImportReport::stale_rows`] and [`ImportOptions::replace_range`].

pub(crate) mod accumulate;
pub(crate) mod mapping;
pub(crate) mod parse;
pub(crate) mod units;

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
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
/// against existing rows one day either side.
///
/// # It only means something on the first run
///
/// Writes are last-write-wins, so once an import has run, the stored rows it
/// would compare against **are its own previous output**. Comparing against
/// yourself proves nothing, and worse, it actively misleads: after a correct
/// boundary fix, run 2 finds its nights bit-identical to run 1's rows one day
/// over and reports a mismatch in the opposite direction, so the fix-and-re-run
/// loop oscillates forever while each run argues confidently against the last.
///
/// Self-comparison is therefore detected and reported as such
/// ([`Self::SelfComparison`]), never as agreement. A false green light here
/// would be worse than no check at all, because it retires the one question
/// this importer cannot otherwise answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SleepDayShift {
    /// Nothing to compare against: either no `SleepTotal` rows are stored at
    /// all, or none sit in a full three-day window around an imported night.
    NoComparableRows,
    /// The stored rows are (mostly) this importer's own earlier output, so the
    /// boundary was not independently checked.
    SelfComparison { compared_days: usize },
    /// A genuine comparison against rows this import did not write.
    ///
    /// All three means share one denominator — only days with a stored row at
    /// `D−1`, `D` *and* `D+1` are counted — because comparing means taken over
    /// different day sets is how a single sparse row produces a confident
    /// verdict out of nothing.
    Compared {
        compared_days: usize,
        /// Mean |ours(D) − existing(D−1)|.
        prev_day: f64,
        /// Mean |ours(D) − existing(D)|.
        same_day: f64,
        /// Mean |ours(D) − existing(D+1)|.
        next_day: f64,
    },
}

/// A neighboring day must fit at least this much better, proportionally,
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
    /// Same-day fits, or no neighbor fits materially better.
    Agrees,
    /// A neighboring day fits decisively better; `offset` is which one.
    Mismatch { offset: i8 },
}

/// Index of the smallest mean, preferring same-day on a tie: equal fits carry
/// no evidence of a shift, and the same day is the null hypothesis.
fn best_index(means: [f64; 3]) -> usize {
    // Same-day first so `min_by`, which keeps the first minimum, wins ties.
    [1usize, 0, 2]
        .into_iter()
        .min_by(|a, b| means[*a].total_cmp(&means[*b]))
        .unwrap_or(1)
}

/// Day offset each mean in the `[prev, same, next]` triple was measured at.
const OFFSET_FOR_INDEX: [i8; 3] = [-1, 0, 1];

impl SleepDayShift {
    /// The offset that fits best, or `None` when no real comparison happened.
    #[must_use]
    pub fn best_offset(&self) -> Option<i8> {
        let Self::Compared {
            prev_day,
            same_day,
            next_day,
            ..
        } = *self
        else {
            return None;
        };
        Some(OFFSET_FOR_INDEX[best_index([prev_day, same_day, next_day])])
    }

    /// Whether the sleep-day boundary agrees with the rows already stored.
    ///
    /// Deliberately conservative. This warning tells the operator to change a
    /// constant and re-import a decade of history, so it fires only when a
    /// neighboring day fits both proportionally and absolutely better — never
    /// on a tie, never on noise, and never against this import's own output.
    #[must_use]
    pub fn verdict(&self) -> SleepDayVerdict {
        let Self::Compared {
            prev_day,
            same_day,
            next_day,
            ..
        } = *self
        else {
            return SleepDayVerdict::Unverified;
        };
        let means = [prev_day, same_day, next_day];
        let best = best_index(means);
        if best == 1 {
            return SleepDayVerdict::Agrees;
        }
        if means[best] < same_day * MATERIAL_SHIFT_RATIO
            && (same_day - means[best]) >= MATERIAL_SHIFT_HOURS
        {
            SleepDayVerdict::Mismatch {
                offset: OFFSET_FOR_INDEX[best],
            }
        } else {
            SleepDayVerdict::Agrees
        }
    }
}

/// Rows already in the store, of a kind this run produced and inside that
/// kind's imported date range, that this run did **not** rewrite.
///
/// Upsert never deletes, so a re-run whose fix changes a row's *key* rather
/// than its *value* — a sleep-day boundary change, or re-mapping a name to a
/// different [`MetricKind`] — leaves the old rows behind holding known-wrong
/// values on days that now belong to a different night, or to none. Values look
/// entirely normal and `rows_overwritten` stays high, so nothing else in the
/// report would hint at them.
///
/// **Not proof of that, though.** `daily_metric` records no provenance
/// (healthie-1ru), so a row here is equally consistent with a day the live HAE
/// push covered and this export simply does not. Deleting is therefore opt-in
/// and the report names both possibilities.
#[derive(Debug, Clone)]
pub struct StaleRows {
    pub kind: MetricKind,
    pub count: usize,
    /// A few examples, for going and looking.
    pub sample_dates: Vec<NaiveDate>,
}

/// How an import should treat rows it did not rewrite.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOptions {
    /// Delete pre-existing rows inside the imported range that this run did not
    /// produce, instead of only reporting them.
    ///
    /// Off by default: deleting real data is the operator's decision, not a
    /// silent consequence of re-running an import.
    pub replace_range: bool,
}

/// Number of stale dates listed per kind before the report stops enumerating.
const STALE_SAMPLE_LIMIT: usize = 5;

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
    /// Rows this run did not rewrite — see [`StaleRows`].
    pub stale_rows: Vec<StaleRows>,
    /// How many of those were deleted (non-zero only with
    /// [`ImportOptions::replace_range`]).
    pub stale_rows_deleted: usize,
    /// Whether the export closed every element it opened.
    ///
    /// `false` means the file ended early — an interrupted transfer of a
    /// multi-gigabyte export cut at a record boundary looks well-formed and
    /// parses without error. Whatever it held has still been imported.
    pub document_closed: bool,
    /// `--replace-range` was asked for and refused because the export was
    /// truncated. Rows that look stale against a partial file are mostly rows
    /// the file simply does not reach.
    pub replace_range_refused_truncated: bool,
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
/// # What re-running does and does not guarantee
///
/// `(kind, date)` upserts last-write-wins, so a re-run whose fix changes a
/// row's **value** — a unit-scale correction, an aggregation change — fully
/// replaces the old figure. That case is idempotent.
///
/// A re-run whose fix changes a row's **key** is not. Shifting the sleep-day
/// boundary, or re-mapping a name onto a different [`MetricKind`], moves
/// nights onto different dates; nothing deletes what was written at the old
/// ones, so rows survive holding known-wrong values. Those are reported as
/// [`ImportReport::stale_rows`], and [`ImportOptions::replace_range`] deletes
/// them.
///
/// # Errors
/// Returns [`DomainError::Db`] on database failure.
pub async fn persist_import<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    parsed: ParsedExport,
    options: ImportOptions,
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

    persist_quarantine(&txn, &stats, now).await?;

    // Rows inside the imported range that this run did NOT write. On a first
    // import there are none; after a key-changing fix they are the old, wrong
    // placements that upsert alone can never reach.
    let stale = match min_date.zip(max_date) {
        Some(range) => find_stale_rows(&txn, &resolved.rows, range).await?,
        None => Vec::new(),
    };
    // A file truncated mid-transfer parses cleanly if the cut fell on a record
    // boundary — it just stops early. Importing what it holds is fine; deleting
    // rows because they are "missing" from it is not, since most of the export
    // may simply not be there.
    let replace_range_refused_truncated = options.replace_range && !stats.document_closed;
    let stale_rows_deleted = if options.replace_range && !replace_range_refused_truncated {
        delete_stale_rows(&txn, &stale).await?
    } else {
        0
    };

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
        stale_rows: summarize_stale(&stale),
        stale_rows_deleted,
        document_closed: stats.document_closed,
        replace_range_refused_truncated,
    })
}

/// Write one quarantine row per uncurated name seen this run.
async fn persist_quarantine<C: ConnectionTrait>(
    db: &C,
    stats: &ImportStats,
    now: chrono::DateTime<chrono::Utc>,
) -> DomainResult<()> {
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
        drop_other_dates(db, raw_name, sample.date).await?;
        upsert_quarantine(db, raw_name, sample.date, &point, now).await?;
    }
    Ok(())
}

/// Rows this run did not write, of a kind it produced and inside **that kind's
/// own** date range.
///
/// Scoped per kind rather than over one range shared by every kind, because the
/// ranges differ wildly: a decade of `HeartRate` alongside two `Weight`
/// readings would otherwise stretch the weight window across the whole decade
/// and sweep in every weigh-in the live push landed in between.
///
/// Even per-kind this cannot be exact — `daily_metric` has no column recording
/// which intake wrote a row (healthie-1ru), so a gap this export does not cover
/// is indistinguishable from a row an earlier import misplaced. The caller
/// reports both possibilities rather than asserting one.
async fn find_stale_rows<C: ConnectionTrait>(
    db: &C,
    produced: &[PendingRow],
    (from, to): (NaiveDate, NaiveDate),
) -> DomainResult<Vec<(MetricKind, NaiveDate, i32)>> {
    let mut ranges: BTreeMap<MetricKind, (NaiveDate, NaiveDate)> = BTreeMap::new();
    for row in produced {
        ranges
            .entry(row.kind)
            .and_modify(|(lo, hi)| {
                *lo = (*lo).min(row.date);
                *hi = (*hi).max(row.date);
            })
            .or_insert((row.date, row.date));
    }
    let kinds: BTreeSet<MetricKind> = ranges.keys().copied().collect();
    let written: HashSet<(MetricKind, NaiveDate)> =
        produced.iter().map(|r| (r.kind, r.date)).collect();

    Ok(daily_metric::Entity::find()
        // The global range only narrows what the database returns; each row is
        // then held to its own kind's range below.
        .filter(daily_metric::Column::Kind.is_in(kinds))
        .filter(daily_metric::Column::Date.between(from, to))
        .all(db)
        .await?
        .into_iter()
        .filter(|row| {
            ranges
                .get(&row.kind)
                .is_some_and(|(lo, hi)| row.date >= *lo && row.date <= *hi)
                && !written.contains(&(row.kind, row.date))
        })
        .map(|row| (row.kind, row.date, row.id))
        .collect())
}

/// Bound on ids per `IN (…)` statement.
///
/// `SQLITE_MAX_VARIABLE_NUMBER` defaults to 32766; a decade of orphaned rows
/// can exceed that, and the statement would fail. It fails *safe* — the
/// transaction rolls back — but that would make the flag unusable on precisely
/// the case it exists for.
const DELETE_CHUNK: usize = 1_000;

/// Delete the given rows, returning how many the database reports removing.
///
/// The count comes from `rows_affected` rather than the length of the input:
/// on the one operation here that destroys data, the number shown to the
/// operator should be what happened, not what was intended.
async fn delete_stale_rows<C: ConnectionTrait>(
    db: &C,
    stale: &[(MetricKind, NaiveDate, i32)],
) -> DomainResult<usize> {
    let mut deleted = 0u64;
    for chunk in stale.chunks(DELETE_CHUNK) {
        let ids: Vec<i32> = chunk.iter().map(|(_, _, id)| *id).collect();
        let result = daily_metric::Entity::delete_many()
            .filter(daily_metric::Column::Id.is_in(ids))
            .exec(db)
            .await?;
        deleted += result.rows_affected;
    }
    Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
}

fn summarize_stale(stale: &[(MetricKind, NaiveDate, i32)]) -> Vec<StaleRows> {
    let mut by_kind: BTreeMap<MetricKind, Vec<NaiveDate>> = BTreeMap::new();
    for (kind, date, _) in stale {
        by_kind.entry(*kind).or_default().push(*date);
    }
    by_kind
        .into_iter()
        .map(|(kind, mut dates)| {
            dates.sort_unstable();
            StaleRows {
                kind,
                count: dates.len(),
                sample_dates: dates.into_iter().take(STALE_SAMPLE_LIMIT).collect(),
            }
        })
        .collect()
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
        // Load-bearing, NOT a redundant re-check of the SQL filter above:
        // SQLite's LIKE is ASCII-case-insensitive, so `starts_with(HK_PREFIX)`
        // also matches `hk…`. ADR-0006 §6's claim that this scope can never
        // reach an HAE row rests on these exact-match Rust predicates.
        .filter(|row| {
            let name_now_handled =
                is_recognized_hk_name(&row.raw_name) || map_sleep_stage(&row.raw_name).is_some();
            name_now_handled && quarantined_for_its_name(&row.raw_point)
        })
        .map(|row| row.id)
        .collect();
    // Bounded by the number of distinct uncurated names (~hundreds), so one
    // statement would fit — chunked anyway to match `delete_stale_rows`, since
    // "it happens to be small" is not a property anything enforces.
    let mut cleared = 0u64;
    for chunk in stale.chunks(DELETE_CHUNK) {
        let result = quarantined_metric::Entity::delete_many()
            .filter(quarantined_metric::Column::Id.is_in(chunk.to_vec()))
            .exec(db)
            .await?;
        cleared += result.rows_affected;
    }
    Ok(usize::try_from(cleared).unwrap_or(usize::MAX))
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

/// Fraction of a comparison that may land on bit-identical values before we
/// conclude we are reading our own output.
///
/// Two independent computations of a night — Apple's own rollup versus an
/// interval union reconstructed from raw segments — essentially never agree to
/// the last bit, and essentially never across a majority of days. A
/// preponderance of exact matches is not agreement; it is almost certainly a
/// mirror.
///
/// Not a proof: live rows that happened to carry this import's nights one day
/// over, with coinciding values, would also trip it and suppress a real
/// mismatch. That needs bit-identity across most days and is not achievable
/// against real HAE figures — and the failure direction is the safe one, since
/// it withholds a verdict rather than issuing a false green light.
const SELF_COMPARISON_FRACTION: f64 = 0.5;

/// Mean absolute difference between reconstructed `SleepTotal` values and
/// existing rows at day offsets −1, 0 and +1.
///
/// Only days with a stored row at all three offsets are counted, so the three
/// means share a denominator. Means taken over different day sets are not
/// comparable to each other: one stored row beside imported nights of 8/2/8
/// hours would otherwise yield `prev = 0.0` against `same = 6.0` and a
/// confident verdict conjured out of a single row.
fn sleep_day_shift(rows: &[PendingRow], existing: &BTreeMap<NaiveDate, f64>) -> SleepDayShift {
    if existing.is_empty() {
        return SleepDayShift::NoComparableRows;
    }
    let mut sums = [0.0f64; 3];
    let mut exact_matches = [0usize; 3];
    let mut compared_days = 0usize;

    for row in rows.iter().filter(|r| r.kind == MetricKind::SleepTotal) {
        let (Some(prev), Some(next)) = (row.date.pred_opt(), row.date.succ_opt()) else {
            continue;
        };
        let (Some(before), Some(on), Some(after)) = (
            existing.get(&prev),
            existing.get(&row.date),
            existing.get(&next),
        ) else {
            continue;
        };
        for (slot, stored) in [(0usize, before), (1, on), (2, after)] {
            let diff = (row.value - stored).abs();
            sums[slot] += diff;
            if diff == 0.0 {
                exact_matches[slot] += 1;
            }
        }
        compared_days += 1;
    }

    if compared_days == 0 {
        return SleepDayShift::NoComparableRows;
    }
    // A day tally; f64 is exact well past this.
    #[allow(clippy::cast_precision_loss)]
    let n = compared_days as f64;
    let means = [sums[0] / n, sums[1] / n, sums[2] / n];

    // If the offset that "fits best" fits because it is reading rows this
    // importer wrote on an earlier run, there is nothing here to learn.
    #[allow(clippy::cast_precision_loss)]
    let exact_at_best = exact_matches[best_index(means)] as f64;
    if exact_at_best > n * SELF_COMPARISON_FRACTION {
        return SleepDayShift::SelfComparison { compared_days };
    }

    SleepDayShift::Compared {
        compared_days,
        prev_day: means[0],
        same_day: means[1],
        next_day: means[2],
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, EntityTrait};

    use super::{
        ImportOptions, ImportReport, SleepDayShift, SleepDayVerdict, parse_export_reader,
        persist_import,
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
        persist_import(db, parsed, ImportOptions::default())
            .await
            .expect("persist")
    }

    async fn import_replacing<C: ConnectionTrait + sea_orm::TransactionTrait>(
        db: &C,
        xml: &str,
    ) -> ImportReport {
        let parsed = parse_export_reader(xml.as_bytes()).expect("parse");
        persist_import(
            db,
            parsed,
            ImportOptions {
                replace_range: true,
            },
        )
        .await
        .expect("persist")
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

        // Counts alone would pass with a non-deterministic winner silently
        // rewriting values on every run; assert the values themselves.
        let mut actual: Vec<(MetricKind, f64, Option<f64>, Option<f64>)> = rows
            .iter()
            .map(|r| (r.kind, r.value, r.min, r.max))
            .collect();
        actual.sort_by_key(|a| a.0);
        let weight = 220.462_262_184_877_6;
        let expected = [
            // Weight is AvgMinMax, so a lone reading is its own spread.
            (MetricKind::Weight, weight, Some(weight), Some(weight)),
            (MetricKind::HeartRate, 70.0, Some(50.0), Some(90.0)),
            (MetricKind::SleepTotal, 6.0, None, None),
            (MetricKind::SleepCore, 6.0, None, None),
        ];
        let mut expected = expected.to_vec();
        expected.sort_by_key(|a| a.0);
        for ((kind, value, min, max), (ek, ev, emin, emax)) in actual.iter().zip(&expected) {
            assert_eq!(kind, ek);
            assert!(close(*value, *ev), "{kind:?}: {value} != {ev}");
            assert_eq!((*min, *max), (*emin, *emax), "{kind:?} spread");
        }
    }

    /// A re-run whose fix changes a row's KEY rather than its value cannot be
    /// idempotent: nothing deletes what sat at the old key. A boundary change
    /// is exactly that case, and the stale rows hold plausible-looking values
    /// on days that now belong to a different night.
    #[tokio::test]
    async fn key_changing_rerun_leaves_stale_rows_and_reports_them() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        // Stand in for an earlier import that placed this night a day over.
        for day in ["2026-07-26", "2026-07-27"] {
            daily_metric::ActiveModel {
                kind: Set(MetricKind::SleepTotal),
                date: Set(date(day)),
                value: Set(9.9),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(&db)
            .await
            .expect("seed");
        }

        let xml = r#"<HealthData>
          <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                  value="HKCategoryValueSleepAnalysisAsleepCore"
                  startDate="2026-07-25 23:00:00 -0700" endDate="2026-07-26 05:00:00 -0700"/>
          <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                  value="HKCategoryValueSleepAnalysisAsleepCore"
                  startDate="2026-07-27 23:00:00 -0700" endDate="2026-07-28 05:00:00 -0700"/>
        </HealthData>"#;
        let report = import(&db, xml).await;

        // 2026-07-27 sits inside the imported range but was not produced.
        let stale = report
            .stale_rows
            .iter()
            .find(|s| s.kind == MetricKind::SleepTotal)
            .expect("stale sleep-total row reported");
        assert_eq!(stale.count, 1);
        assert_eq!(stale.sample_dates, vec![date("2026-07-27")]);
        assert_eq!(report.stale_rows_deleted, 0, "reporting must not delete");
        assert!(
            daily_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .iter()
                .any(|r| r.date == date("2026-07-27") && (r.value - 9.9).abs() < 1e-9),
            "the stale row is still there — that is the point"
        );

        // …and --replace-range is the remedy.
        let replaced = import_replacing(&db, xml).await;
        assert_eq!(replaced.stale_rows_deleted, 1);
        assert!(
            !daily_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .iter()
                .any(|r| r.date == date("2026-07-27")),
            "replace-range must remove the row this run did not write"
        );
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
        // Existing rows carrying our night's ~6 hours one day EARLIER, plus
        // unrelated neighbors so the comparison has something to reject. The
        // 5.97 is deliberately NOT 6.0: an independently-computed rollup never
        // matches a reconstruction bit-for-bit, and exact equality is what
        // flags a self-comparison.
        for (day, hours) in [
            ("2026-07-27", 5.97),
            ("2026-07-28", 1.2),
            ("2026-07-29", 1.1),
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

    /// Three consecutive nights, so every night has both neighbors and the
    /// comparison can actually run.
    const THREE_NIGHTS: &str = r#"<HealthData>
      <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
              value="HKCategoryValueSleepAnalysisAsleepCore"
              startDate="2026-07-26 23:00:00 -0700" endDate="2026-07-27 05:00:00 -0700"/>
      <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
              value="HKCategoryValueSleepAnalysisAsleepCore"
              startDate="2026-07-27 23:00:00 -0700" endDate="2026-07-28 06:00:00 -0700"/>
      <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
              value="HKCategoryValueSleepAnalysisAsleepCore"
              startDate="2026-07-28 22:30:00 -0700" endDate="2026-07-29 05:30:00 -0700"/>
    </HealthData>"#;

    /// Once an import has run, last-write-wins means the rows it would compare
    /// against are its own output. Reporting that as agreement would retire the
    /// one question this importer cannot otherwise answer — and after a genuine
    /// boundary fix it would actively argue against the correct answer, making
    /// the prescribed fix-and-re-run loop oscillate forever.
    #[tokio::test]
    async fn reimport_detects_that_it_is_reading_its_own_output() {
        let db = test_db().await;
        import(&db, THREE_NIGHTS).await;
        let second = import(&db, THREE_NIGHTS).await;

        assert!(
            matches!(second.sleep_day_shift, SleepDayShift::SelfComparison { .. }),
            "expected a self-comparison, got {:?}",
            second.sleep_day_shift
        );
        assert_eq!(
            second.sleep_day_shift.verdict(),
            SleepDayVerdict::Unverified,
            "comparing against yourself verifies nothing"
        );
        assert_eq!(second.sleep_day_shift.best_offset(), None);
    }

    #[test]
    fn a_marginal_neighbor_is_not_a_mismatch() {
        // Better, but neither proportionally nor absolutely decisive.
        let shift = SleepDayShift::Compared {
            compared_days: 100,
            prev_day: 0.30,
            same_day: 0.34,
            next_day: 9.0,
        };
        assert_eq!(shift.best_offset(), Some(-1));
        assert_eq!(shift.verdict(), SleepDayVerdict::Agrees);

        // Decisively better on both counts.
        let shift = SleepDayShift::Compared {
            compared_days: 100,
            prev_day: 0.10,
            same_day: 4.80,
            next_day: 9.0,
        };
        assert_eq!(shift.verdict(), SleepDayVerdict::Mismatch { offset: -1 });

        // An exact tie is not evidence of anything.
        let shift = SleepDayShift::Compared {
            compared_days: 100,
            prev_day: 0.0,
            same_day: 0.0,
            next_day: 0.0,
        };
        assert_eq!(shift.best_offset(), Some(0));
        assert_eq!(shift.verdict(), SleepDayVerdict::Agrees);
    }

    /// The three means must share a denominator. Accumulating each over
    /// whatever days happen to have a stored row lets one sparse row produce a
    /// confident verdict out of nothing.
    #[tokio::test]
    async fn a_single_sparse_row_cannot_produce_a_verdict() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        // Exactly one stored row, on the middle night only.
        daily_metric::ActiveModel {
            kind: Set(MetricKind::SleepTotal),
            date: Set(date("2026-07-28")),
            value: Set(2.0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("seed");

        let report = import(&db, THREE_NIGHTS).await;
        assert_eq!(
            report.sleep_day_shift,
            SleepDayShift::NoComparableRows,
            "no night has all three neighbors stored, so nothing is comparable"
        );
        assert_eq!(
            report.sleep_day_shift.verdict(),
            SleepDayVerdict::Unverified
        );
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

    /// Stale detection is scoped to each kind's OWN date range. One range
    /// shared across every kind would stretch a sparse metric's window over the
    /// whole import: a decade of heart rate beside two weigh-ins would sweep in
    /// every weight row the live push landed in between — and `--replace-range`
    /// would then delete real data.
    #[tokio::test]
    async fn stale_detection_does_not_bleed_across_kinds() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        // A live-ingested weigh-in in the middle of the import's overall span,
        // but outside the range this import's own weight rows cover.
        daily_metric::ActiveModel {
            kind: Set(MetricKind::Weight),
            date: Set(date("2026-07-15")),
            value: Set(198.0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("seed live weigh-in");

        // Heart rate spans 07-01..07-28; weight appears only on 07-28.
        let report = import(
            &db,
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierHeartRate" unit="count/min"
                      startDate="2026-07-01 08:00:00 -0700" endDate="2026-07-01 08:00:00 -0700" value="60"/>
              <Record type="HKQuantityTypeIdentifierHeartRate" unit="count/min"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="62"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 06:00:00 -0700" endDate="2026-07-28 06:00:00 -0700" value="200"/>
            </HealthData>"#,
        )
        .await;

        assert!(
            report.stale_rows.is_empty(),
            "the 07-15 weigh-in is outside the weight rows' own range and must not be flagged: \
             {:?}",
            report.stale_rows
        );
        assert!(
            daily_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .iter()
                .any(|r| r.kind == MetricKind::Weight && r.date == date("2026-07-15")),
            "the live weigh-in must survive"
        );
    }

    /// A transfer cut at a record boundary parses without error and simply
    /// stops early. Importing what it holds is fine; DELETING rows because they
    /// are "missing" from it is not — most of the export may not be there.
    #[tokio::test]
    async fn replace_range_is_refused_on_a_truncated_export() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        for day in ["2026-07-10", "2026-07-20"] {
            daily_metric::ActiveModel {
                kind: Set(MetricKind::HeartRate),
                date: Set(date(day)),
                value: Set(61.0),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(&db)
            .await
            .expect("seed");
        }

        // Cut at a clean record boundary: no `</HealthData>`.
        let truncated = r#"<HealthData locale="en_US">
          <Record type="HKQuantityTypeIdentifierHeartRate" unit="count/min"
                  startDate="2026-07-01 08:00:00 -0700" endDate="2026-07-01 08:00:00 -0700" value="60"/>
          <Record type="HKQuantityTypeIdentifierHeartRate" unit="count/min"
                  startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="62"/>"#;

        let report = import_replacing(&db, truncated).await;

        assert!(!report.document_closed, "the fixture is truncated");
        assert!(report.replace_range_refused_truncated);
        assert_eq!(report.stale_rows_deleted, 0);
        assert_eq!(
            daily_metric::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .iter()
                .filter(|r| r.kind == MetricKind::HeartRate)
                .count(),
            4,
            "both seeded rows survive alongside the two imported ones"
        );
        // The partial data still landed — leniency is only withheld from the
        // destructive half.
        assert_eq!(report.records_curated, 2);
    }

    #[tokio::test]
    async fn a_complete_export_is_not_flagged_as_truncated() {
        let db = test_db().await;
        let report = import(&db, FIXTURE).await;
        assert!(report.document_closed);
        assert!(!report.replace_range_refused_truncated);
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
