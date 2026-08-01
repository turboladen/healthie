//! Folding `export.xml`'s per-reading records into one `daily_metric` row per
//! `(kind, date)`.
//!
//! # Why this exists
//!
//! The live HAE push arrives **pre-aggregated**: Apple's app has already
//! reduced a day to a `qty`, or to an `Avg`/`Min`/`Max` triple. `export.xml`
//! has not — it is raw per-reading `<Record>` elements, thousands per day for
//! something like heart rate. The backfill therefore has to reconstruct the
//! rollup HAE gets for free, and reconstructing it *wrong* is silent and
//! permanent: a bad policy yields plausible numbers that quietly poison every
//! trend computed later. [`daily_agg`] is that policy, decided per metric.
//!
//! # Memory
//!
//! Quantity retention is `O(kinds × days)` — one small [`Acc`] per
//! `(kind, date)`, never the readings themselves. For a decade that is ~19
//! kinds × ~4,500 days ≈ 85k entries.
//!
//! Sleep is different and the bound is weaker, so state it honestly: sleep
//! retains *intervals*, coalesced on insert, so retention is `O(stages ×
//! nights × disjoint sleep periods per night)`. It does **not** grow with
//! record count, nor with the number of recording devices — overlapping and
//! adjacent segments merge as they arrive. It is not a hard bound: a source
//! writing alternating one-minute segments would push a night from the typical
//! ~10–40 disjoint periods to ~500. [`DEGENERATE_INTERVAL_COUNT`] makes that
//! case loud rather than letting it quietly consume memory.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, FixedOffset, NaiveDate, Timelike};

use crate::entities::daily_metric::MetricKind;

/// Local hour at which one "sleep day" ends and the next begins.
///
/// A night spans midnight, so some rule must decide which calendar date it
/// belongs to. Segments are attributed by their **start**, whole and never
/// split, with anything starting at or after this hour counted toward the
/// following day — so a night beginning 23:14 and a 02:10 deep-sleep segment
/// from the same night both land on the morning you woke up.
///
/// 18:00 is Apple's own 6PM–6PM sleep day, so a backfilled row agrees with what
/// the Health app displays. The alternatives were worse: attributing by start
/// with no shift splits every ordinary night across two rows; attributing by
/// *end* still strands a pre-midnight `Awake` segment on the previous day; a
/// flat 12-hour shift groups nights correctly but pushes every afternoon nap
/// onto the next day.
///
/// Known edge, accepted: a segment starting 17:50 and ending 06:00 lands on the
/// earlier day. If backfilled sleep ever proves to sit one day off from live
/// HAE rows, this constant is the first and only thing to change — the import
/// report's day-shift check exists to detect exactly that.
pub(crate) const SLEEP_DAY_BOUNDARY_HOUR: u32 = 18;

/// Disjoint-interval count for one `(night, stage)` above which we warn.
///
/// Well-behaved sources produce a few dozen. Hitting this means some app is
/// writing pathologically fragmented segments, which is the one input that can
/// break the memory profile documented above — so it is surfaced, loudly, with
/// the date and stage to go look at. Nothing is dropped.
pub(crate) const DEGENERATE_INTERVAL_COUNT: usize = 1_000;

/// Source label recorded when a record carries no `sourceName`.
const UNATTRIBUTED_SOURCE: &str = "(unattributed)";

/// How one day's readings of a given [`MetricKind`] collapse into a single
/// `daily_metric` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DailyAgg {
    /// Total the day's readings — for counters that accumulate, where each
    /// record is a slice of the day.
    ///
    /// Not a plain sum: a day's records are first totalled **per source**, and
    /// the largest single source wins. Apple's export keeps every device's
    /// version of the same day, so an iPhone and a Watch that both counted
    /// steps would otherwise roughly double the total. This makes Sum-kind
    /// values a *lower bound* — see the module docs on `apple_health`.
    Sum,
    /// Mean into `value`, plus the day's spread into `min`/`max`. Mirrors the
    /// `Avg`/`Min`/`Max` triple HAE sends for these on the live path, so live
    /// and backfilled rows are directly comparable.
    AvgMinMax,
    /// Mean into `value`; `min`/`max` left NULL.
    Mean,
    /// The day's maximum reading wins.
    Max,
}

/// The per-metric daily rollup policy (Steve, 2026-07-31). Standing preference:
/// keep the spread, discard nothing — `min`/`max` already exist on the row, so
/// preserving the day's range is free and lets the trend layer decide later.
///
/// Returns `None` for the six sleep kinds, which are folded from timed segments
/// on a separate path and never reach this function. The match is exhaustive
/// with no wildcard on purpose: a new [`MetricKind`] fails to compile here until
/// someone decides its policy.
// Arms that share a policy are kept apart on purpose: each carries the
// reasoning for *that* metric, and merging them by outcome would collapse four
// independent decisions into one anonymous list.
#[allow(clippy::match_same_arms)]
pub(crate) fn daily_agg(kind: MetricKind) -> Option<DailyAgg> {
    Some(match kind {
        // Accumulating counters.
        MetricKind::Steps
        | MetricKind::ActiveEnergy
        | MetricKind::ExerciseMinutes
        | MetricKind::StandMinutes
        | MetricKind::WalkingDistance => DailyAgg::Sum,

        // Sampled continuously all day, where the spread is clinically the
        // point — a desaturation dip or an HR spike is invisible in a mean.
        MetricKind::HeartRate
        | MetricKind::Spo2
        | MetricKind::RespiratoryRate
        | MetricKind::Hrv => DailyAgg::AvgMinMax,

        // Gait/mobility, sampled sporadically, no meaningful daily spread.
        MetricKind::WalkingSpeed
        | MetricKind::GaitAsymmetry
        | MetricKind::GaitDoubleSupport
        | MetricKind::StepLength => DailyAgg::Mean,

        // Weight/BodyFat: mean in `value`, the day's low/high in `min`/`max`.
        // Chosen over `Last` so a stray evening weigh-in cannot silently become
        // the day's official weight (a fake 2-3 lb step in the trend), and over
        // plain `Mean` so the morning/evening spread is not thrown away.
        MetricKind::Weight | MetricKind::BodyFat => DailyAgg::AvgMinMax,

        // CardioRecovery: mean across the day's workouts, worst/best retained.
        // A widening spread is itself a signal, so it is kept, not collapsed.
        MetricKind::CardioRecovery => DailyAgg::AvgMinMax,

        // BreathingDisturbances: `max` holds the worst figure of the day, so a
        // bad night can never be averaged into unremarkability, while `value`
        // still carries the typical-night baseline needed to judge whether a
        // bad night is actually unusual.
        MetricKind::BreathingDisturbances => DailyAgg::AvgMinMax,

        // Vo2Max: demonstrated best of the day. Chosen with the trade-off on
        // the table — `Max` ratchets upward and will NOT show a genuine
        // decline. Accepted because Apple's per-workout estimate is noisy
        // enough that the daily best is the more trustworthy figure. If
        // declines ever need to surface, revisit here first.
        MetricKind::Vo2Max => DailyAgg::Max,

        // RestingHeartRate: Apple normally emits exactly one per day, so every
        // policy coincides; `Mean` only decides the rare two-reading day.
        MetricKind::RestingHeartRate => DailyAgg::Mean,

        // Folded from timed segments, not from daily readings.
        MetricKind::SleepTotal
        | MetricKind::SleepDeep
        | MetricKind::SleepRem
        | MetricKind::SleepCore
        | MetricKind::SleepAwake
        | MetricKind::TimeInBed => return None,
    })
}

/// The calendar date a sleep segment starting at `start` belongs to.
pub(crate) fn sleep_date(start: DateTime<FixedOffset>) -> NaiveDate {
    let date = start.date_naive();
    if start.hour() >= SLEEP_DAY_BOUNDARY_HOUR {
        date.succ_opt().unwrap_or(date)
    } else {
        date
    }
}

/// A half-open span of wall-clock seconds, `start < end` by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Interval {
    start_secs: i64,
    end_secs: i64,
}

impl Interval {
    /// `None` for an empty or inverted span — a zero-length or backwards
    /// segment carries no duration and must not be folded in.
    pub(crate) fn new(start_secs: i64, end_secs: i64) -> Option<Self> {
        (end_secs > start_secs).then_some(Self {
            start_secs,
            end_secs,
        })
    }

    fn secs(self) -> i64 {
        self.end_secs - self.start_secs
    }
}

/// A sorted, disjoint, non-adjacent set of [`Interval`]s.
///
/// Segments coalesce as they are inserted, which is what makes the sleep fold
/// correct rather than merely plausible: Apple's export retains every device's
/// account of the same night, so two watches — or a watch and a sleep app —
/// covering 23:00–06:00 must yield seven hours, not fourteen. Summing raw
/// durations would silently double the night.
#[derive(Debug, Default)]
pub(crate) struct IntervalSet {
    intervals: Vec<Interval>,
    warned_degenerate: bool,
}

impl IntervalSet {
    /// Insert `iv`, merging it with every interval it overlaps or touches.
    fn insert(&mut self, iv: Interval) {
        // First index that could merge: everything before ends strictly earlier
        // than iv starts, so it is neither overlapping nor adjacent.
        let lo = self
            .intervals
            .partition_point(|x| x.end_secs < iv.start_secs);
        // First index strictly after: starts later than iv ends.
        let hi = self
            .intervals
            .partition_point(|x| x.start_secs <= iv.end_secs);

        let mut merged = iv;
        if lo < hi {
            merged.start_secs = merged.start_secs.min(self.intervals[lo].start_secs);
            merged.end_secs = merged.end_secs.max(self.intervals[hi - 1].end_secs);
        }
        self.intervals.splice(lo..hi, std::iter::once(merged));
    }

    fn total_secs(&self) -> i64 {
        self.intervals.iter().map(|i| i.secs()).sum()
    }

    fn hours(&self) -> f64 {
        // i64 seconds in a day-scale range; f64 is exact well past this.
        #[allow(clippy::cast_precision_loss)]
        let secs = self.total_secs() as f64;
        secs / 3600.0
    }

    fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    /// Number of disjoint intervals retained — the quantity the module's memory
    /// bound is stated in.
    pub(crate) fn len(&self) -> usize {
        self.intervals.len()
    }
}

/// Running rollup for one `(kind, date)`.
#[derive(Debug)]
struct Acc {
    agg: DailyAgg,
    sum: f64,
    count: u64,
    min: f64,
    max: f64,
    /// Timestamp of the latest reading seen, so `source` is the chronologically
    /// last device rather than whichever record the file happened to list last
    /// — `export.xml` record order is not guaranteed.
    last_ts: DateTime<FixedOffset>,
    source: Option<String>,
    /// Per-source totals, kept only for [`DailyAgg::Sum`] kinds, where several
    /// devices double-count the same day.
    by_source: Option<HashMap<String, f64>>,
}

impl Acc {
    fn new(agg: DailyAgg, value: f64, ts: DateTime<FixedOffset>, source: Option<&str>) -> Self {
        let mut acc = Self {
            agg,
            sum: 0.0,
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            last_ts: ts,
            source: None,
            by_source: (agg == DailyAgg::Sum).then(HashMap::new),
        };
        acc.fold(value, ts, source);
        acc
    }

    fn fold(&mut self, value: f64, ts: DateTime<FixedOffset>, source: Option<&str>) {
        self.sum += value;
        self.count += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        if self.count == 1 || supersedes_source(ts, source, self.last_ts, self.source.as_deref()) {
            self.last_ts = ts;
            self.source = source.map(str::to_owned);
        }
        if let Some(by_source) = &mut self.by_source {
            *by_source
                .entry(source.unwrap_or(UNATTRIBUTED_SOURCE).to_owned())
                .or_insert(0.0) += value;
        }
    }

    fn mean(&self) -> f64 {
        // `count` is a reading tally; f64 is exact to 2^53.
        #[allow(clippy::cast_precision_loss)]
        let n = self.count as f64;
        self.sum / n
    }

    /// `(value, min, max)` for the row, per this kind's policy.
    fn resolve(&self) -> (f64, Option<f64>, Option<f64>) {
        match self.agg {
            DailyAgg::Sum => (self.winning_source_total(), None, None),
            DailyAgg::AvgMinMax => (self.mean(), Some(self.min), Some(self.max)),
            DailyAgg::Mean => (self.mean(), None, None),
            DailyAgg::Max => (self.max, None, None),
        }
    }

    /// The source that contributed the most for the day, and its total.
    ///
    /// `None` for non-`Sum` kinds, which do not track per-source totals. Both
    /// the stored value and the credited name are derived from this one helper,
    /// so they can never end up describing different devices.
    ///
    /// Ties break on the name so that re-importing unchanged data cannot pick a
    /// different winner: `by_source` is a `HashMap`, whose iteration order is
    /// unspecified, and `max_by` keeps the *last* of several equal maxima.
    fn winning_source(&self) -> Option<(&str, f64)> {
        self.by_source
            .as_ref()?
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(name, total)| (name.as_str(), *total))
    }

    /// The largest single source's total for the day.
    fn winning_source_total(&self) -> f64 {
        self.winning_source().map_or(self.sum, |(_, total)| total)
    }

    /// Source name credited on the row: for Sum kinds the device that actually
    /// won the day, otherwise the chronologically latest.
    fn resolve_source(&self) -> Option<String> {
        let Some((name, _)) = self.winning_source() else {
            return self.source.clone();
        };
        // The winning bucket is the records that carried no `sourceName`, so the
        // stored value came from no named device. Falling back to some other
        // device's name here would credit it with a total it did not contribute.
        (name != UNATTRIBUTED_SOURCE).then(|| name.to_owned())
    }
}

/// Running sleep fold for one night.
#[derive(Debug, Default)]
struct SleepAcc {
    stages: BTreeMap<MetricKind, StageAcc>,
    /// Union of every asleep-class segment, whatever its stage — Apple emits no
    /// total, so `SleepTotal` is derived from this.
    asleep: StageAcc,
}

/// One sleep sub-metric's intervals and the device credited for them.
///
/// Source is tracked per stage rather than once per night because each stage
/// becomes its own row: crediting them all to whichever segment happened to be
/// latest would name, on a `SleepCore` row, a bed sensor that only ever
/// reported `InBed` — the same misattribution the Sum path goes to trouble to
/// avoid, and the reason `SleepTotal` counts only asleep-class segments.
#[derive(Debug, Default)]
struct StageAcc {
    set: IntervalSet,
    source: Option<String>,
    last_ts: Option<DateTime<FixedOffset>>,
}

impl StageAcc {
    /// Fold in one segment, taking its source if it supersedes the current one.
    fn insert(&mut self, start: DateTime<FixedOffset>, interval: Interval, source: Option<&str>) {
        if self
            .last_ts
            .is_none_or(|prev| supersedes_source(start, source, prev, self.source.as_deref()))
        {
            self.last_ts = Some(start);
            self.source = source.map(str::to_owned);
        }
        self.set.insert(interval);
    }
}

/// One resolved `daily_metric` row, ready to persist.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingRow {
    pub(crate) kind: MetricKind,
    pub(crate) date: NaiveDate,
    pub(crate) value: f64,
    pub(crate) min: Option<f64>,
    pub(crate) max: Option<f64>,
    pub(crate) source: Option<String>,
}

/// Per-kind span of the values actually written, for the import report.
///
/// This is the tripwire for a scaling mistake. `export.xml`'s percent-typed
/// quantities are undocumented as to whether they arrive as `0.97` or `97`, so
/// rather than guess with a heuristic, the run prints what it wrote: a spo2
/// column reading `0.91 .. 0.99 %` instead of `91 .. 99 %` is obvious on sight,
/// before anything downstream consumes it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KindSummary {
    pub(crate) days: usize,
    pub(crate) value_min: f64,
    pub(crate) value_max: f64,
}

/// How much a Sum kind's multi-source de-duplication is actually discarding.
///
/// `sourceName` is free-form text a user can rename, so de-duplication cannot
/// be assumed reliable — this reports its magnitude instead of asserting its
/// correctness. A `worst_ratio` near 1.0 means the rule was inert; near 2.0
/// means it just prevented a doubled day.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SumSourceSummary {
    pub(crate) days_multi_source: usize,
    pub(crate) mean_ratio: f64,
    pub(crate) worst_ratio: f64,
}

/// Everything one pass over `export.xml` accumulates.
#[derive(Debug, Default)]
pub(crate) struct Accumulator {
    quantities: HashMap<(MetricKind, NaiveDate), Acc>,
    sleep: HashMap<NaiveDate, SleepAcc>,
}

/// The resolved output of a fold: rows plus the summaries the report needs.
pub(crate) struct Resolved {
    pub(crate) rows: Vec<PendingRow>,
    pub(crate) per_kind: BTreeMap<MetricKind, KindSummary>,
    pub(crate) sum_sources: BTreeMap<MetricKind, SumSourceSummary>,
}

impl Accumulator {
    /// Fold one quantity reading, already converted to `kind`'s canonical unit.
    pub(crate) fn fold_quantity(
        &mut self,
        kind: MetricKind,
        agg: DailyAgg,
        ts: DateTime<FixedOffset>,
        value: f64,
        source: Option<&str>,
    ) {
        // The local calendar day the reading belongs to — taken from the
        // record's own offset, never UTC-converted (ADR-0005 §5).
        let date = ts.date_naive();
        self.quantities
            .entry((kind, date))
            .and_modify(|acc| acc.fold(value, ts, source))
            .or_insert_with(|| Acc::new(agg, value, ts, source));
    }

    /// Fold one sleep segment into its night.
    pub(crate) fn fold_sleep_segment(
        &mut self,
        stage: Option<MetricKind>,
        counts_as_asleep: bool,
        start: DateTime<FixedOffset>,
        interval: Interval,
        source: Option<&str>,
    ) {
        let date = sleep_date(start);
        let night = self.sleep.entry(date).or_default();

        if counts_as_asleep {
            night.asleep.insert(start, interval, source);
            // Checked explicitly: undifferentiated legacy segments carry no
            // stage, so this union is the ONLY set they grow. Omitting it would
            // leave the one input that can break the memory profile unwatched
            // for exactly the oldest years of history.
            warn_if_degenerate(&mut night.asleep.set, date, None, source);
        }
        if let Some(stage) = stage {
            let acc = night.stages.entry(stage).or_default();
            acc.insert(start, interval, source);
            warn_if_degenerate(&mut acc.set, date, Some(stage), source);
        }
    }

    /// Largest number of disjoint intervals retained for any one
    /// `(night, stage)`. Exists so the memory bound can be asserted rather than
    /// merely asserted about.
    #[cfg(test)]
    pub(crate) fn max_intervals_retained(&self) -> usize {
        self.sleep
            .values()
            .flat_map(|night| {
                night
                    .stages
                    .values()
                    .map(|acc| acc.set.len())
                    .chain(std::iter::once(night.asleep.set.len()))
            })
            .max()
            .unwrap_or(0)
    }

    /// Number of distinct `(kind, date)` quantity rollups held.
    #[cfg(test)]
    pub(crate) fn quantity_entries(&self) -> usize {
        self.quantities.len()
    }

    /// Collapse everything folded so far into rows, ordered by kind then date
    /// so a run's output is deterministic.
    pub(crate) fn resolve(&self) -> Resolved {
        let mut rows = Vec::with_capacity(self.quantities.len());
        let mut ratios: BTreeMap<MetricKind, Vec<f64>> = BTreeMap::new();

        for ((kind, date), acc) in &self.quantities {
            let (value, min, max) = acc.resolve();
            rows.push(PendingRow {
                kind: *kind,
                date: *date,
                value,
                min,
                max,
                source: acc.resolve_source(),
            });
            if let Some(by_source) = &acc.by_source
                && by_source.len() > 1
            {
                let winner = acc.winning_source_total();
                if winner > 0.0 {
                    ratios.entry(*kind).or_default().push(acc.sum / winner);
                }
            }
        }

        for (date, night) in &self.sleep {
            for (stage, acc) in &night.stages {
                if !acc.set.is_empty() {
                    rows.push(sleep_row(
                        *stage,
                        *date,
                        acc.set.hours(),
                        acc.source.clone(),
                    ));
                }
            }
            if !night.asleep.set.is_empty() {
                rows.push(sleep_row(
                    MetricKind::SleepTotal,
                    *date,
                    night.asleep.set.hours(),
                    night.asleep.source.clone(),
                ));
            }
        }

        rows.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.date.cmp(&b.date)));
        let per_kind = summarize(&rows);
        Resolved {
            rows,
            per_kind,
            sum_sources: summarize_sources(&ratios),
        }
    }
}

/// Whether a reading at `ts` from `source` should take over the credited source
/// from the one currently held.
///
/// The latest reading wins. An exact timestamp tie is decided by name rather
/// than by arrival order, because `export.xml` record order is not guaranteed
/// and the credited device must not depend on it. Spelled out rather than left
/// to `Option`'s derived ordering, under which `None < Some(_)` would let an
/// unattributed record displace a real device and then never let one displace
/// it back.
///
/// Shared by the quantity and sleep folds so the two cannot drift apart — they
/// did once.
fn supersedes_source(
    ts: DateTime<FixedOffset>,
    source: Option<&str>,
    current_ts: DateTime<FixedOffset>,
    current_source: Option<&str>,
) -> bool {
    match ts.cmp(&current_ts) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => match (source, current_source) {
            (Some(new), Some(current)) => new < current,
            (Some(_), None) => true,
            (None, _) => false,
        },
        std::cmp::Ordering::Less => false,
    }
}

/// Warn once per set when a night's disjoint-interval count leaves the range
/// the module's memory profile is stated over. `stage` is `None` for the
/// combined asleep union.
fn warn_if_degenerate(
    set: &mut IntervalSet,
    date: NaiveDate,
    stage: Option<MetricKind>,
    source: Option<&str>,
) {
    if set.len() >= DEGENERATE_INTERVAL_COUNT && !set.warned_degenerate {
        set.warned_degenerate = true;
        tracing::warn!(
            %date,
            ?stage,
            intervals = set.len(),
            source = source.unwrap_or(UNATTRIBUTED_SOURCE),
            "fragmented sleep segments — retention for this night is far above typical"
        );
    }
}

fn sleep_row(kind: MetricKind, date: NaiveDate, hours: f64, source: Option<String>) -> PendingRow {
    PendingRow {
        kind,
        date,
        value: hours,
        min: None,
        max: None,
        source,
    }
}

fn summarize(rows: &[PendingRow]) -> BTreeMap<MetricKind, KindSummary> {
    let mut per_kind: BTreeMap<MetricKind, KindSummary> = BTreeMap::new();
    for row in rows {
        let entry = per_kind.entry(row.kind).or_insert(KindSummary {
            days: 0,
            value_min: f64::INFINITY,
            value_max: f64::NEG_INFINITY,
        });
        entry.days += 1;
        entry.value_min = entry.value_min.min(row.value);
        entry.value_max = entry.value_max.max(row.value);
    }
    per_kind
}

fn summarize_sources(
    ratios: &BTreeMap<MetricKind, Vec<f64>>,
) -> BTreeMap<MetricKind, SumSourceSummary> {
    ratios
        .iter()
        .map(|(kind, values)| {
            // Ratio counts are day tallies; f64 is exact well past this.
            #[allow(clippy::cast_precision_loss)]
            let n = values.len() as f64;
            let summary = SumSourceSummary {
                days_multi_source: values.len(),
                mean_ratio: values.iter().sum::<f64>() / n,
                worst_ratio: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            };
            (*kind, summary)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sea_orm::strum::IntoEnumIterator as _;

    use super::{
        Accumulator, DailyAgg, Interval, IntervalSet, PendingRow, SLEEP_DAY_BOUNDARY_HOUR,
        daily_agg, sleep_date,
    };
    use crate::{
        entities::daily_metric::MetricKind, services::metrics::parse_local, test_support::date,
    };

    fn ts(s: &str) -> chrono::DateTime<chrono::FixedOffset> {
        parse_local(s).expect("valid offset timestamp literal")
    }

    fn segment(start: &str, end: &str) -> (chrono::DateTime<chrono::FixedOffset>, Interval) {
        let (s, e) = (ts(start), ts(end));
        (
            s,
            Interval::new(s.timestamp(), e.timestamp()).expect("non-empty segment"),
        )
    }

    fn row_for(rows: &[PendingRow], kind: MetricKind) -> &PendingRow {
        rows.iter()
            .find(|r| r.kind == kind)
            .unwrap_or_else(|| panic!("no row for {kind:?}"))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The full policy, kind by kind. The exhaustive match guarantees every
    /// variant is *decided*; this asserts it was decided the way Steve chose.
    #[test]
    fn daily_agg_table_is_exhaustive_and_matches_policy() {
        let expected = [
            (MetricKind::Weight, DailyAgg::AvgMinMax),
            (MetricKind::BodyFat, DailyAgg::AvgMinMax),
            (MetricKind::Vo2Max, DailyAgg::Max),
            (MetricKind::RestingHeartRate, DailyAgg::Mean),
            (MetricKind::HeartRate, DailyAgg::AvgMinMax),
            (MetricKind::Hrv, DailyAgg::AvgMinMax),
            (MetricKind::Spo2, DailyAgg::AvgMinMax),
            (MetricKind::BreathingDisturbances, DailyAgg::AvgMinMax),
            (MetricKind::RespiratoryRate, DailyAgg::AvgMinMax),
            (MetricKind::CardioRecovery, DailyAgg::AvgMinMax),
            (MetricKind::ActiveEnergy, DailyAgg::Sum),
            (MetricKind::Steps, DailyAgg::Sum),
            (MetricKind::ExerciseMinutes, DailyAgg::Sum),
            (MetricKind::WalkingDistance, DailyAgg::Sum),
            (MetricKind::StandMinutes, DailyAgg::Sum),
            (MetricKind::WalkingSpeed, DailyAgg::Mean),
            (MetricKind::GaitAsymmetry, DailyAgg::Mean),
            (MetricKind::GaitDoubleSupport, DailyAgg::Mean),
            (MetricKind::StepLength, DailyAgg::Mean),
        ];
        assert_eq!(expected.len(), 19, "all non-sleep kinds must be listed");
        for (kind, agg) in expected {
            assert_eq!(daily_agg(kind), Some(agg), "{kind:?}");
        }
        for kind in [
            MetricKind::SleepTotal,
            MetricKind::SleepDeep,
            MetricKind::SleepRem,
            MetricKind::SleepCore,
            MetricKind::SleepAwake,
            MetricKind::TimeInBed,
        ] {
            assert_eq!(
                daily_agg(kind),
                None,
                "{kind:?} folds from segments, not daily readings"
            );
        }
        // Nothing escaped the table above.
        assert_eq!(MetricKind::iter().count(), 25);
    }

    #[test]
    fn sleep_day_boundary_groups_a_night_onto_the_waking_day() {
        // Before the boundary: the segment's own date.
        assert_eq!(
            sleep_date(ts("2026-07-28 17:59:00 -0700")),
            date("2026-07-28")
        );
        // At and after it: the following day, so a night groups forward.
        assert_eq!(
            sleep_date(ts("2026-07-28 18:00:00 -0700")),
            date("2026-07-29")
        );
        assert_eq!(
            sleep_date(ts("2026-07-27 23:50:00 -0700")),
            date("2026-07-28")
        );
        // Small hours already belong to the waking day.
        assert_eq!(
            sleep_date(ts("2026-07-28 02:10:00 -0700")),
            date("2026-07-28")
        );
        // An afternoon nap stays on its own day.
        assert_eq!(
            sleep_date(ts("2026-07-28 14:00:00 -0700")),
            date("2026-07-28")
        );
        // Month and year rollover.
        assert_eq!(
            sleep_date(ts("2026-07-31 22:00:00 -0700")),
            date("2026-08-01")
        );
        assert_eq!(
            sleep_date(ts("2026-12-31 22:00:00 -0700")),
            date("2027-01-01")
        );
        assert_eq!(SLEEP_DAY_BOUNDARY_HOUR, 18);
    }

    /// The whole reason sleep folds intervals instead of summing durations.
    #[test]
    fn interval_set_merges_overlapping_adjacent_and_keeps_disjoint() {
        // Two devices recording the identical night must not double it.
        let mut set = IntervalSet::default();
        set.insert(Interval::new(0, 3_600).unwrap());
        set.insert(Interval::new(0, 3_600).unwrap());
        assert_eq!(set.len(), 1);
        assert!(
            close(set.hours(), 1.0),
            "duplicate coverage is not additive"
        );

        // Partial overlap merges to the union, not the sum.
        let mut set = IntervalSet::default();
        set.insert(Interval::new(0, 3_600).unwrap());
        set.insert(Interval::new(1_800, 5_400).unwrap());
        assert_eq!(set.len(), 1);
        assert!(close(set.hours(), 1.5));

        // Touching intervals coalesce.
        let mut set = IntervalSet::default();
        set.insert(Interval::new(0, 3_600).unwrap());
        set.insert(Interval::new(3_600, 7_200).unwrap());
        assert_eq!(set.len(), 1);
        assert!(close(set.hours(), 2.0));

        // Genuinely disjoint spans stay separate and do add up.
        let mut set = IntervalSet::default();
        set.insert(Interval::new(0, 3_600).unwrap());
        set.insert(Interval::new(7_200, 10_800).unwrap());
        assert_eq!(set.len(), 2);
        assert!(close(set.hours(), 2.0));

        // Out-of-order insertion, and one span swallowing several others.
        let mut set = IntervalSet::default();
        for (s, e) in [(7_200, 10_800), (0, 3_600), (14_400, 18_000)] {
            set.insert(Interval::new(s, e).unwrap());
        }
        assert_eq!(set.len(), 3);
        set.insert(Interval::new(0, 18_000).unwrap());
        assert_eq!(set.len(), 1, "a covering span absorbs the rest");
        assert!(close(set.hours(), 5.0));
    }

    #[test]
    fn empty_and_inverted_intervals_are_refused() {
        assert!(Interval::new(100, 100).is_none());
        assert!(Interval::new(200, 100).is_none());
    }

    /// `SleepTotal` is the union of asleep stages, so overlapping segments from
    /// different sources make it *less* than the sum of its stages. Correct,
    /// and pinned here because a later reader would otherwise assume the
    /// stages add up to the total.
    #[test]
    fn sleep_total_may_be_less_than_the_sum_of_its_stages() {
        let mut acc = Accumulator::default();
        // Watch says 23:00–01:00 was core; a sleep app says 00:00–02:00 was
        // deep. One hour is claimed by both.
        let (start, iv) = segment("2026-07-27 23:00:00 -0700", "2026-07-28 01:00:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::SleepCore), true, start, iv, Some("Watch"));
        let (start, iv) = segment("2026-07-28 00:00:00 -0700", "2026-07-28 02:00:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::SleepDeep), true, start, iv, Some("App"));

        let rows = acc.resolve().rows;
        let core = row_for(&rows, MetricKind::SleepCore).value;
        let deep = row_for(&rows, MetricKind::SleepDeep).value;
        let total = row_for(&rows, MetricKind::SleepTotal).value;
        assert!(close(core, 2.0) && close(deep, 2.0));
        assert!(
            close(total, 3.0),
            "total is the union (23:00-02:00 = 3h), got {total}"
        );
        assert!(total < core + deep, "stages overlap, so they over-count");
        // All of it belongs to the waking day.
        assert_eq!(
            row_for(&rows, MetricKind::SleepTotal).date,
            date("2026-07-28")
        );
    }

    /// In-bed and awake time are not sleep and must never inflate the total.
    #[test]
    fn in_bed_and_awake_do_not_feed_sleep_total() {
        let mut acc = Accumulator::default();
        let (start, iv) = segment("2026-07-27 22:00:00 -0700", "2026-07-28 06:00:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::TimeInBed), false, start, iv, None);
        let (start, iv) = segment("2026-07-28 03:00:00 -0700", "2026-07-28 03:30:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::SleepAwake), false, start, iv, None);
        let (start, iv) = segment("2026-07-28 00:00:00 -0700", "2026-07-28 02:00:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::SleepCore), true, start, iv, None);

        let rows = acc.resolve().rows;
        assert!(close(row_for(&rows, MetricKind::TimeInBed).value, 8.0));
        assert!(close(row_for(&rows, MetricKind::SleepAwake).value, 0.5));
        assert!(
            close(row_for(&rows, MetricKind::SleepTotal).value, 2.0),
            "only asleep stages count toward the total"
        );
    }

    /// Undifferentiated legacy segments have no stage row but still total.
    #[test]
    fn undifferentiated_sleep_totals_without_a_stage_row() {
        let mut acc = Accumulator::default();
        let (start, iv) = segment("2026-07-27 23:00:00 -0700", "2026-07-28 05:30:00 -0700");
        acc.fold_sleep_segment(None, true, start, iv, Some("iPhone"));
        let rows = acc.resolve().rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, MetricKind::SleepTotal);
        assert!(close(rows[0].value, 6.5));
    }

    #[test]
    fn avg_min_max_keeps_the_days_spread() {
        let mut acc = Accumulator::default();
        for (t, v) in [
            ("2026-07-28 08:00:00 -0700", 60.0),
            ("2026-07-28 12:00:00 -0700", 50.0),
            ("2026-07-28 20:00:00 -0700", 100.0),
        ] {
            acc.fold_quantity(
                MetricKind::HeartRate,
                DailyAgg::AvgMinMax,
                ts(t),
                v,
                Some("Watch"),
            );
        }
        let rows = acc.resolve().rows;
        assert_eq!(rows.len(), 1);
        assert!(close(rows[0].value, 70.0));
        assert_eq!((rows[0].min, rows[0].max), (Some(50.0), Some(100.0)));
    }

    #[test]
    fn mean_and_max_policies_leave_spread_null() {
        let mut acc = Accumulator::default();
        for v in [40.0, 50.0] {
            acc.fold_quantity(
                MetricKind::Vo2Max,
                DailyAgg::Max,
                ts("2026-07-28 08:00:00 -0700"),
                v,
                None,
            );
            acc.fold_quantity(
                MetricKind::WalkingSpeed,
                DailyAgg::Mean,
                ts("2026-07-28 08:00:00 -0700"),
                v,
                None,
            );
        }
        let rows = acc.resolve().rows;
        let vo2 = row_for(&rows, MetricKind::Vo2Max);
        assert!(close(vo2.value, 50.0) && vo2.min.is_none() && vo2.max.is_none());
        let speed = row_for(&rows, MetricKind::WalkingSpeed);
        assert!(close(speed.value, 45.0) && speed.min.is_none());
    }

    /// Readings land on the local calendar day of their own offset — a
    /// late-evening reading must not be UTC-shifted onto tomorrow.
    #[test]
    fn quantity_uses_local_calendar_day() {
        let mut acc = Accumulator::default();
        acc.fold_quantity(
            MetricKind::Weight,
            DailyAgg::AvgMinMax,
            ts("2026-07-28 23:40:00 -0700"),
            200.0,
            None,
        );
        assert_eq!(acc.resolve().rows[0].date, date("2026-07-28"));
    }

    /// Apple keeps every device's account of the same day; summing them
    /// blindly would roughly double a step count.
    #[test]
    fn multi_source_sum_takes_the_winning_source() {
        let mut acc = Accumulator::default();
        let t = ts("2026-07-28 09:00:00 -0700");
        acc.fold_quantity(MetricKind::Steps, DailyAgg::Sum, t, 4_000.0, Some("iPhone"));
        acc.fold_quantity(MetricKind::Steps, DailyAgg::Sum, t, 6_000.0, Some("Watch"));
        acc.fold_quantity(MetricKind::Steps, DailyAgg::Sum, t, 3_500.0, Some("Watch"));

        let resolved = acc.resolve();
        assert!(
            close(resolved.rows[0].value, 9_500.0),
            "expected the winning source's total, got {}",
            resolved.rows[0].value
        );
        assert_eq!(resolved.rows[0].source.as_deref(), Some("Watch"));
        let stats = resolved.sum_sources[&MetricKind::Steps];
        assert_eq!(stats.days_multi_source, 1);
        assert!(
            close(stats.worst_ratio, 13_500.0 / 9_500.0),
            "the report must show how much the rule discarded"
        );
    }

    /// The regression risk of the rule above: it must not disturb the ordinary
    /// single-device day, which is nearly all of them.
    #[test]
    fn single_source_sum_is_a_plain_total() {
        let mut acc = Accumulator::default();
        let t = ts("2026-07-28 09:00:00 -0700");
        for v in [1_000.0, 2_000.0, 3_000.0] {
            acc.fold_quantity(MetricKind::Steps, DailyAgg::Sum, t, v, Some("Watch"));
        }
        let resolved = acc.resolve();
        assert!(close(resolved.rows[0].value, 6_000.0));
        assert!(
            resolved.sum_sources.is_empty(),
            "a single-source day is not multi-source and must not be reported as such"
        );
    }

    /// The credited source must be the device that actually contributed the
    /// stored total. When the winning bucket is unattributed records, no device
    /// may be named — crediting the latest-seen one would attribute 8,000 steps
    /// to a device that contributed 500 of them.
    #[test]
    fn unattributed_winner_credits_no_device() {
        let mut acc = Accumulator::default();
        acc.fold_quantity(
            MetricKind::Steps,
            DailyAgg::Sum,
            ts("2026-07-28 09:00:00 -0700"),
            8_000.0,
            None,
        );
        acc.fold_quantity(
            MetricKind::Steps,
            DailyAgg::Sum,
            ts("2026-07-28 10:00:00 -0700"),
            500.0,
            Some("Watch"),
        );
        let rows = acc.resolve().rows;
        assert!(close(rows[0].value, 8_000.0));
        assert_eq!(
            rows[0].source, None,
            "the 8,000 came from no named device, so none may be credited"
        );
    }

    /// `by_source` is a `HashMap`, so an exact tie must not let iteration order
    /// decide the credited device — a re-import would otherwise rewrite
    /// `source` for no input-driven reason.
    #[test]
    fn tied_sources_resolve_deterministically() {
        let credited = |order: [&str; 2]| {
            let mut acc = Accumulator::default();
            for name in order {
                acc.fold_quantity(
                    MetricKind::Steps,
                    DailyAgg::Sum,
                    ts("2026-07-28 09:00:00 -0700"),
                    5_000.0,
                    Some(name),
                );
            }
            acc.resolve().rows[0].source.clone()
        };
        assert_eq!(credited(["Watch", "iPhone"]), credited(["iPhone", "Watch"]));
        assert_eq!(credited(["Watch", "iPhone"]).as_deref(), Some("Watch"));
    }

    /// At an exact timestamp tie the credited source must be order-independent,
    /// and an unattributed record must never erase a real device's name.
    #[test]
    fn same_timestamp_source_tie_break_is_deterministic() {
        let credited = |order: [Option<&str>; 2]| {
            let mut acc = Accumulator::default();
            for source in order {
                acc.fold_quantity(
                    MetricKind::HeartRate,
                    DailyAgg::AvgMinMax,
                    ts("2026-07-28 08:00:00 -0700"),
                    60.0,
                    source,
                );
            }
            acc.resolve().rows[0].source.clone()
        };

        // Two named devices: same answer whichever order they arrive in.
        assert_eq!(
            credited([Some("Watch"), Some("iPhone")]),
            credited([Some("iPhone"), Some("Watch")])
        );
        // A named device always beats no attribution, in either order.
        assert_eq!(credited([Some("Watch"), None]).as_deref(), Some("Watch"));
        assert_eq!(
            credited([None, Some("Watch")]).as_deref(),
            Some("Watch"),
            "an unattributed record must not erase a real device"
        );
    }

    /// `SleepTotal` is built only from asleep-class segments, so its credited
    /// source must come from one. An `InBed` device that contributed no sleep
    /// time must not end up named on the total.
    #[test]
    fn sleep_total_is_credited_to_a_device_that_contributed_sleep() {
        let mut acc = Accumulator::default();
        // A bed sensor logs time in bed, later than the watch's sleep segment.
        let (start, iv) = segment("2026-07-27 22:00:00 -0700", "2026-07-28 06:30:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::TimeInBed), false, start, iv, Some("Bed"));
        let (start, iv) = segment("2026-07-27 23:00:00 -0700", "2026-07-28 05:00:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::SleepCore), true, start, iv, Some("Watch"));
        let (start, iv) = segment("2026-07-28 06:00:00 -0700", "2026-07-28 06:20:00 -0700");
        acc.fold_sleep_segment(Some(MetricKind::SleepAwake), false, start, iv, Some("Bed"));

        let rows = acc.resolve().rows;
        assert_eq!(
            row_for(&rows, MetricKind::SleepTotal).source.as_deref(),
            Some("Watch"),
            "the bed sensor contributed no asleep time and must not be credited"
        );
        assert_eq!(
            row_for(&rows, MetricKind::TimeInBed).source.as_deref(),
            Some("Bed")
        );
        // Every stage is its own row, so every stage needs its own source: the
        // Bed sensor's later Awake segment must not be credited on SleepCore.
        assert_eq!(
            row_for(&rows, MetricKind::SleepCore).source.as_deref(),
            Some("Watch"),
            "the bed sensor contributed no core sleep and must not be credited"
        );
        assert_eq!(
            row_for(&rows, MetricKind::SleepAwake).source.as_deref(),
            Some("Bed")
        );
    }

    /// The sleep fold credits a source too, and must tie-break identically —
    /// this is shared logic precisely because the two paths drifted apart once.
    #[test]
    fn sleep_source_tie_break_matches_the_quantity_path() {
        let credited = |order: [Option<&str>; 2]| {
            let mut acc = Accumulator::default();
            for source in order {
                let (start, iv) = segment("2026-07-27 23:00:00 -0700", "2026-07-28 05:00:00 -0700");
                acc.fold_sleep_segment(Some(MetricKind::SleepCore), true, start, iv, source);
            }
            acc.resolve().rows[0].source.clone()
        };

        assert_eq!(
            credited([Some("Watch"), Some("SleepApp")]),
            credited([Some("SleepApp"), Some("Watch")])
        );
        assert_eq!(
            credited([None, Some("Watch")]).as_deref(),
            Some("Watch"),
            "an unattributed segment must not erase a real device"
        );
        assert_eq!(credited([Some("Watch"), None]).as_deref(), Some("Watch"));
    }

    /// Retention must track distinct `(kind, date)` pairs and disjoint sleep
    /// periods — never the number of records folded.
    #[test]
    fn retention_is_bounded_by_days_not_by_record_count() {
        let mut acc = Accumulator::default();
        let days = ["2026-07-27", "2026-07-28", "2026-07-29"];

        for i in 0..60_000u32 {
            let day = days[i as usize % days.len()];
            let t = ts(&format!("{day} 09:00:00 -0700"));
            acc.fold_quantity(
                MetricKind::HeartRate,
                DailyAgg::AvgMinMax,
                t,
                60.0 + f64::from(i % 30),
                Some("Watch"),
            );
            // Heavily overlapping segments, as several devices would produce.
            let start = ts(&format!("{day} 01:00:00 -0700"));
            let iv = Interval::new(start.timestamp(), start.timestamp() + 3_600).unwrap();
            acc.fold_sleep_segment(Some(MetricKind::SleepCore), true, start, iv, Some("Watch"));
        }

        assert_eq!(
            acc.quantity_entries(),
            days.len(),
            "one rollup per (kind, date), regardless of 60k readings"
        );
        assert_eq!(
            acc.max_intervals_retained(),
            1,
            "60k identical segments coalesce to one disjoint interval"
        );
    }
}
