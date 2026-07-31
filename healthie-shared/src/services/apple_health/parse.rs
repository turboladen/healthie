//! Streaming `export.xml` parse.
//!
//! Apple's export is routinely multi-gigabyte, so the document is never
//! materialized: a [`BufRead`] feeds quick-xml's pull reader, each `<Record>`
//! is folded into the accumulator, and the event buffer is cleared every
//! iteration. No record is retained as such — what outlives one is the
//! accumulator's per-`(kind, date)` rollup plus [`ImportStats`], which keeps at
//! most one verbatim sample per *uncurated name*. Both are bounded by the
//! vocabulary and the date range, never by the number of records.
//!
//! `<Record>` arrives in two shapes and both must be handled — self-closing
//! (`Event::Empty`) and, when it carries `<MetadataEntry/>` children,
//! `Event::Start`. quick-xml only unifies those if `expand_empty_elements` is
//! set, which costs an allocation per element; matching both is free. Elements
//! that are not `Record` (`Workout`, `ActivitySummary`, `Me`, and any child
//! nodes) cost one byte-slice comparison and are skipped.

use std::{borrow::Cow, collections::BTreeMap, io::BufRead};

use chrono::{DateTime, FixedOffset, NaiveDate};
use quick_xml::{
    Reader, XmlVersion,
    events::{BytesStart, Event},
};
use serde_json::{Map, Value};

use super::{
    accumulate::{Accumulator, Interval, daily_agg},
    mapping::{HkMapping, map_hk_name, map_sleep_stage},
    units::convert_to_canonical,
};
use crate::{
    entities::daily_metric::MetricKind,
    error::{DomainError, DomainResult},
    services::metrics::parse_local,
};

/// The only element this parser acts on.
const RECORD: &[u8] = b"Record";

/// How often to emit a progress line; a multi-GB parse takes minutes and should
/// not look hung.
const PROGRESS_EVERY: u64 = 1_000_000;

/// One retained example of an uncurated name, plus how often it was seen.
///
/// Deliberately one sample per name rather than one row per `(name, date)` as
/// the live HAE path uses — see the `apple_health` module docs.
#[derive(Debug, Clone)]
pub(crate) struct QuarantineSample {
    pub(crate) records_seen: u64,
    pub(crate) date: NaiveDate,
    pub(crate) point: Value,
}

/// Counts and discoveries from one pass over the file.
#[derive(Debug, Default)]
pub(crate) struct ImportStats {
    pub(crate) records_read: u64,
    pub(crate) records_curated: u64,
    pub(crate) records_excluded: u64,
    /// Records that carried no usable type, timestamp or value. Counted rather
    /// than silently discarded, so a systematic parse problem is visible.
    pub(crate) records_skipped: u64,
    pub(crate) quarantined: BTreeMap<String, QuarantineSample>,
    /// `(type, unit)` pairs no conversion covers, with how many records each
    /// affected.
    pub(crate) unconvertible: BTreeMap<(String, String), u64>,
}

/// Stream `reader`, folding every `<Record>` into `acc`.
///
/// # Errors
/// Returns [`DomainError::Internal`] on malformed XML or an I/O failure,
/// carrying the byte offset so the problem can be located in a huge file.
/// Individual unusable records are counted in `stats`, not fatal — one bad
/// record must not abandon a decade of history.
pub(crate) fn parse_into<R: BufRead>(
    reader: R,
    acc: &mut Accumulator,
    stats: &mut ImportStats,
) -> DomainResult<()> {
    let mut buf = Vec::new();
    parse_into_buf(reader, &mut buf, acc, stats)
}

/// [`parse_into`] with a caller-supplied event buffer, so tests can assert the
/// buffer stays small — the property that makes a multi-GB file survivable.
///
/// # Errors
/// As [`parse_into`].
pub(crate) fn parse_into_buf<R: BufRead>(
    reader: R,
    buf: &mut Vec<u8>,
    acc: &mut Accumulator,
    stats: &mut ImportStats,
) -> DomainResult<()> {
    let mut xml = Reader::from_reader(reader);
    loop {
        match xml.read_event_into(buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e) | Event::Empty(e)) if e.name().as_ref() == RECORD => {
                stats.records_read += 1;
                fold_record(&e, acc, stats);
                if stats.records_read.is_multiple_of(PROGRESS_EVERY) {
                    tracing::info!(records = stats.records_read, "export.xml parse progress");
                }
            }
            Ok(_) => {}
            Err(err) => {
                return Err(DomainError::Internal(format!(
                    "export.xml: byte {}: {err}",
                    xml.error_position()
                )));
            }
        }
        // quick-xml APPENDS into the caller's buffer and never clears it; without
        // this the buffer grows to the size of the whole file.
        buf.clear();
    }
    Ok(())
}

/// The attributes we care about, borrowed from the event buffer where possible.
#[derive(Default)]
struct RawRecord<'a> {
    ty: Option<Cow<'a, str>>,
    unit: Option<Cow<'a, str>>,
    value: Option<Cow<'a, str>>,
    start: Option<Cow<'a, str>>,
    end: Option<Cow<'a, str>>,
    source: Option<Cow<'a, str>>,
}

impl RawRecord<'_> {
    /// Pull the attributes off one element. Attribute order varies between
    /// exports, so this matches by name and never by position.
    fn extract(e: &BytesStart<'_>) -> Option<Self> {
        let mut rec = Self::default();
        for attr in e.attributes() {
            let attr = attr.ok()?;
            let slot = match attr.key.as_ref() {
                b"type" => &mut rec.ty,
                b"unit" => &mut rec.unit,
                b"value" => &mut rec.value,
                b"startDate" => &mut rec.start,
                b"endDate" => &mut rec.end,
                b"sourceName" => &mut rec.source,
                _ => continue,
            };
            *slot = Some(Cow::Owned(
                attr.normalized_value(XmlVersion::Implicit1_0)
                    .ok()?
                    .into_owned(),
            ));
        }
        Some(rec)
    }

    /// The record verbatim, for quarantine. `reason` records why we could not
    /// store it; `records_seen` is filled in at persist time.
    fn to_json(&self, reason: &str) -> Value {
        let mut map = Map::new();
        for (key, val) in [
            ("type", &self.ty),
            ("unit", &self.unit),
            ("value", &self.value),
            ("startDate", &self.start),
            ("endDate", &self.end),
            ("sourceName", &self.source),
        ] {
            if let Some(v) = val {
                map.insert(key.to_owned(), Value::String(v.clone().into_owned()));
            }
        }
        let mut meta = Map::new();
        meta.insert("reason".to_owned(), Value::String(reason.to_owned()));
        map.insert("_import".to_owned(), Value::Object(meta));
        Value::Object(map)
    }
}

fn fold_record(e: &BytesStart<'_>, acc: &mut Accumulator, stats: &mut ImportStats) {
    let Some(rec) = RawRecord::extract(e) else {
        stats.records_skipped += 1;
        return;
    };
    // A record with no type or no parseable local timestamp cannot be placed on
    // a calendar day at all, and so cannot even be quarantined (the row needs a
    // date). Counted, never silently dropped.
    let (Some(ty), Some(start)) = (
        rec.ty.as_deref(),
        rec.start.as_deref().and_then(parse_local),
    ) else {
        stats.records_skipped += 1;
        return;
    };
    let ty = ty.to_owned();

    match map_hk_name(&ty) {
        HkMapping::Excluded => stats.records_excluded += 1,
        HkMapping::Unknown => {
            quarantine(stats, &ty, "unknown-type", &rec, start.date_naive());
        }
        HkMapping::Curated(kind) => fold_quantity(&ty, kind, start, &rec, acc, stats),
        HkMapping::Sleep => fold_sleep(&rec, start, acc, stats),
    }
}

fn fold_quantity(
    ty: &str,
    kind: MetricKind,
    start: DateTime<FixedOffset>,
    rec: &RawRecord<'_>,
    acc: &mut Accumulator,
    stats: &mut ImportStats,
) {
    let Some(raw) = rec.value.as_deref().and_then(|v| v.parse::<f64>().ok()) else {
        stats.records_skipped += 1;
        return;
    };
    // `"NaN"` and `"inf"` parse successfully and would poison every min/max
    // this kind ever reports.
    if !raw.is_finite() {
        stats.records_skipped += 1;
        return;
    }
    // No unit means no way to know what the number means. Quarantine rather
    // than assume it was already canonical.
    let Some(unit) = rec.unit.as_deref() else {
        quarantine(stats, ty, "missing-unit", rec, start.date_naive());
        return;
    };
    let Some(value) = convert_to_canonical(unit, kind, raw) else {
        *stats
            .unconvertible
            .entry((ty.to_owned(), unit.to_owned()))
            .or_insert(0) += 1;
        quarantine(stats, ty, "unconvertible-unit", rec, start.date_naive());
        return;
    };
    let Some(agg) = daily_agg(kind) else {
        // Unreachable by construction: `map_hk_name` never returns `Curated`
        // for a sleep kind. Reported rather than panicked in a long backfill.
        tracing::error!(?kind, "curated quantity has no aggregation policy");
        stats.records_skipped += 1;
        return;
    };
    acc.fold_quantity(kind, agg, start, value, rec.source.as_deref());
    stats.records_curated += 1;
}

fn fold_sleep(
    rec: &RawRecord<'_>,
    start: DateTime<FixedOffset>,
    acc: &mut Accumulator,
    stats: &mut ImportStats,
) {
    let Some(stage_value) = rec.value.as_deref() else {
        stats.records_skipped += 1;
        return;
    };
    let Some((stage, counts_as_asleep)) = map_sleep_stage(stage_value) else {
        // A sleep stage spelling we do not know: quarantined under the stage
        // string itself, which is the discoverable name here.
        quarantine(
            stats,
            stage_value,
            "unknown-sleep-stage",
            rec,
            start.date_naive(),
        );
        return;
    };
    let Some(interval) = rec
        .end
        .as_deref()
        .and_then(parse_local)
        .and_then(|end| Interval::new(start.timestamp(), end.timestamp()))
    else {
        // Zero-length, backwards, or missing end: no duration to contribute.
        stats.records_skipped += 1;
        return;
    };
    acc.fold_sleep_segment(
        stage,
        counts_as_asleep,
        start,
        interval,
        rec.source.as_deref(),
    );
    stats.records_curated += 1;
}

fn quarantine(
    stats: &mut ImportStats,
    name: &str,
    reason: &str,
    rec: &RawRecord<'_>,
    date: NaiveDate,
) {
    let entry = stats
        .quarantined
        .entry(name.to_owned())
        .or_insert_with(|| QuarantineSample {
            records_seen: 0,
            date,
            point: rec.to_json(reason),
        });
    entry.records_seen += 1;
}

#[cfg(test)]
mod tests {
    use super::{ImportStats, parse_into, parse_into_buf};
    use crate::{
        entities::daily_metric::MetricKind,
        services::apple_health::accumulate::{Accumulator, PendingRow},
        test_support::date,
    };

    fn parse(xml: &str) -> (Vec<PendingRow>, ImportStats) {
        let mut acc = Accumulator::default();
        let mut stats = ImportStats::default();
        parse_into(xml.as_bytes(), &mut acc, &mut stats).expect("fixture parses");
        (acc.resolve().rows, stats)
    }

    fn row_for(rows: &[PendingRow], kind: MetricKind) -> &PendingRow {
        rows.iter()
            .find(|r| r.kind == kind)
            .unwrap_or_else(|| panic!("no row for {kind:?}"))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn scalar_record_converts_to_canonical_unit() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBodyMass" sourceName="Withings" unit="kg"
                      startDate="2026-07-28 06:14:02 -0700" endDate="2026-07-28 06:14:02 -0700" value="100"/>
            </HealthData>"#,
        );
        assert_eq!(stats.records_read, 1);
        assert_eq!(stats.records_curated, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, MetricKind::Weight);
        assert!(
            close(rows[0].value, 220.462_262_184_877_6),
            "kg must become lb"
        );
        assert_eq!(rows[0].source.as_deref(), Some("Withings"));
    }

    /// export.xml is per-reading where HAE is pre-aggregated; several readings
    /// in one day must collapse to one row carrying the day's spread.
    #[test]
    fn multiple_readings_in_a_day_collapse_to_one_row() {
        let (rows, _) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Watch" unit="count/min"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="50"/>
              <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Watch" unit="count/min"
                      startDate="2026-07-28 12:00:00 -0700" endDate="2026-07-28 12:00:00 -0700" value="60"/>
              <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Watch" unit="count/min"
                      startDate="2026-07-28 20:00:00 -0700" endDate="2026-07-28 20:00:00 -0700" value="100"/>
            </HealthData>"#,
        );
        assert_eq!(rows.len(), 1);
        assert!(close(rows[0].value, 70.0));
        assert_eq!((rows[0].min, rows[0].max), (Some(50.0), Some(100.0)));
    }

    #[test]
    fn sum_kind_totals_the_day() {
        let (rows, _) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierStepCount" sourceName="Watch" unit="count"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:10:00 -0700" value="1200"/>
              <Record type="HKQuantityTypeIdentifierStepCount" sourceName="Watch" unit="count"
                      startDate="2026-07-28 09:00:00 -0700" endDate="2026-07-28 09:10:00 -0700" value="800"/>
            </HealthData>"#,
        );
        assert!(close(row_for(&rows, MetricKind::Steps).value, 2_000.0));
    }

    /// ADR-0005 §5: the record's own offset decides the day. UTC-converting a
    /// 23:40 -0700 reading would move it to the next day.
    #[test]
    fn late_evening_reading_stays_on_its_local_day() {
        let (rows, _) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 23:40:00 -0700" endDate="2026-07-28 23:40:00 -0700" value="200"/>
            </HealthData>"#,
        );
        assert_eq!(rows[0].date, date("2026-07-28"));
    }

    /// A night spanning midnight must land whole on the waking day, per stage,
    /// with the total derived from the union of asleep stages.
    #[test]
    fn sleep_segments_fold_per_stage_onto_the_waking_day() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                      value="HKCategoryValueSleepAnalysisInBed"
                      startDate="2026-07-27 22:30:00 -0700" endDate="2026-07-28 06:30:00 -0700"/>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                      value="HKCategoryValueSleepAnalysisAsleepCore"
                      startDate="2026-07-27 23:00:00 -0700" endDate="2026-07-28 01:00:00 -0700"/>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                      value="HKCategoryValueSleepAnalysisAsleepDeep"
                      startDate="2026-07-28 01:00:00 -0700" endDate="2026-07-28 02:30:00 -0700"/>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                      value="HKCategoryValueSleepAnalysisAsleepREM"
                      startDate="2026-07-28 02:30:00 -0700" endDate="2026-07-28 04:00:00 -0700"/>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Watch"
                      value="HKCategoryValueSleepAnalysisAwake"
                      startDate="2026-07-28 04:00:00 -0700" endDate="2026-07-28 04:15:00 -0700"/>
            </HealthData>"#,
        );
        assert_eq!(stats.records_curated, 5);
        for (kind, hours) in [
            (MetricKind::TimeInBed, 8.0),
            (MetricKind::SleepCore, 2.0),
            (MetricKind::SleepDeep, 1.5),
            (MetricKind::SleepRem, 1.5),
            (MetricKind::SleepAwake, 0.25),
            // 23:00 → 04:00 asleep, contiguous across three stages.
            (MetricKind::SleepTotal, 5.0),
        ] {
            let row = row_for(&rows, kind);
            assert!(
                close(row.value, hours),
                "{kind:?} = {} want {hours}",
                row.value
            );
            assert_eq!(
                row.date,
                date("2026-07-28"),
                "{kind:?} belongs to the waking day"
            );
        }
    }

    /// Pre-iOS-16 exports have no stage breakdown; a decade of history contains
    /// years of these and they must still produce a total.
    #[test]
    fn legacy_undifferentiated_sleep_still_totals() {
        let (rows, _) = parse(
            r#"<HealthData>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="iPhone"
                      value="HKCategoryValueSleepAnalysisAsleep"
                      startDate="2026-07-27 23:00:00 -0700" endDate="2026-07-28 05:30:00 -0700"/>
            </HealthData>"#,
        );
        assert_eq!(
            rows.len(),
            1,
            "no stage row exists for undifferentiated sleep"
        );
        assert_eq!(rows[0].kind, MetricKind::SleepTotal);
        assert!(close(rows[0].value, 6.5));
    }

    #[test]
    fn unknown_type_quarantines_verbatim_and_counts() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierDietaryWater" unit="mL"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="240"/>
              <Record type="HKQuantityTypeIdentifierDietaryWater" unit="mL"
                      startDate="2026-07-29 08:00:00 -0700" endDate="2026-07-29 08:00:00 -0700" value="500"/>
            </HealthData>"#,
        );
        assert!(
            rows.is_empty(),
            "an uncurated name must not reach daily_metric"
        );
        let sample = &stats.quarantined["HKQuantityTypeIdentifierDietaryWater"];
        assert_eq!(sample.records_seen, 2, "every record is counted");
        assert_eq!(
            sample.date,
            date("2026-07-28"),
            "the first sighting is retained"
        );
        assert_eq!(sample.point["value"], "240");
        assert_eq!(sample.point["_import"]["reason"], "unknown-type");
    }

    /// A unit we cannot convert must never be stored as if it were canonical.
    #[test]
    fn unconvertible_unit_quarantines_instead_of_coercing() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="mmHg"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="118"/>
            </HealthData>"#,
        );
        assert!(rows.is_empty(), "118 mmHg must not be written as 118 lb");
        assert_eq!(
            stats.unconvertible[&(
                "HKQuantityTypeIdentifierBodyMass".to_owned(),
                "mmHg".to_owned()
            )],
            1
        );
        assert_eq!(
            stats.quarantined["HKQuantityTypeIdentifierBodyMass"].point["_import"]["reason"],
            "unconvertible-unit"
        );
    }

    /// Stone is a real mass unit some locales export, and it converts — this
    /// pins that so the test above cannot silently start covering it.
    #[test]
    fn stone_converts_to_pounds() {
        let (rows, _) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="st"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="16"/>
            </HealthData>"#,
        );
        assert!(close(rows[0].value, 224.0));
    }

    /// The rest of the day still lands — one bad record does not void a kind.
    #[test]
    fn partial_day_keeps_the_convertible_records() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 07:00:00 -0700" endDate="2026-07-28 07:00:00 -0700" value="200"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="furlong"
                      startDate="2026-07-28 12:00:00 -0700" endDate="2026-07-28 12:00:00 -0700" value="999"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 19:00:00 -0700" endDate="2026-07-28 19:00:00 -0700" value="204"/>
            </HealthData>"#,
        );
        assert_eq!(rows.len(), 1);
        assert!(
            close(rows[0].value, 202.0),
            "mean of the two usable readings"
        );
        assert_eq!((rows[0].min, rows[0].max), (Some(200.0), Some(204.0)));
        assert_eq!(stats.records_curated, 2);
        assert!(
            stats
                .quarantined
                .contains_key("HKQuantityTypeIdentifierBodyMass")
        );
    }

    #[test]
    fn missing_unit_quarantines_rather_than_assuming_canonical() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBodyMass"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="80"/>
            </HealthData>"#,
        );
        assert!(rows.is_empty());
        assert_eq!(
            stats.quarantined["HKQuantityTypeIdentifierBodyMass"].point["_import"]["reason"],
            "missing-unit"
        );
    }

    /// Excluded names keep quarantine exceptional: seen, declined, silent.
    #[test]
    fn excluded_type_is_neither_stored_nor_quarantined() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBasalEnergyBurned" unit="Cal"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="1800"/>
            </HealthData>"#,
        );
        assert!(rows.is_empty());
        assert!(stats.quarantined.is_empty());
        assert_eq!(stats.records_excluded, 1);
    }

    #[test]
    fn malformed_records_are_counted_and_do_not_abort_the_parse() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record sourceName="X" unit="lb" startDate="2026-07-28 08:00:00 -0700" value="1"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb" value="2"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="not-a-date" endDate="2026-07-28 08:00:00 -0700" value="3"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="oops"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="NaN"/>
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 09:00:00 -0700" endDate="2026-07-28 09:00:00 -0700" value="201"/>
            </HealthData>"#,
        );
        assert_eq!(stats.records_read, 6);
        assert_eq!(stats.records_skipped, 5);
        assert_eq!(stats.records_curated, 1);
        assert_eq!(rows.len(), 1);
        assert!(
            close(rows[0].value, 201.0),
            "a NaN reading must not poison the day"
        );
    }

    #[test]
    fn zero_length_and_backwards_sleep_segments_are_skipped() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" value="HKCategoryValueSleepAnalysisAsleepCore"
                      startDate="2026-07-28 01:00:00 -0700" endDate="2026-07-28 01:00:00 -0700"/>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" value="HKCategoryValueSleepAnalysisAsleepCore"
                      startDate="2026-07-28 03:00:00 -0700" endDate="2026-07-28 02:00:00 -0700"/>
            </HealthData>"#,
        );
        assert!(rows.is_empty());
        assert_eq!(stats.records_skipped, 2);
    }

    #[test]
    fn unknown_sleep_stage_quarantines_under_its_own_name() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKCategoryTypeIdentifierSleepAnalysis" value="HKCategoryValueSleepAnalysisAsleepFuture"
                      startDate="2026-07-28 01:00:00 -0700" endDate="2026-07-28 02:00:00 -0700"/>
            </HealthData>"#,
        );
        assert!(rows.is_empty());
        assert_eq!(
            stats.quarantined["HKCategoryValueSleepAnalysisAsleepFuture"].records_seen,
            1
        );
    }

    /// quick-xml reports `<Record/>` as `Empty` but `<Record>…</Record>` as
    /// `Start`; a record carrying `<MetadataEntry/>` children takes the second
    /// shape and must parse identically.
    #[test]
    fn record_with_children_parses_like_a_self_closing_one() {
        let (rows, stats) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Watch" unit="count/min"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="61">
                <MetadataEntry key="HKMetadataKeyHeartRateMotionContext" value="1"/>
                <HeartRateVariabilityMetadataList>
                  <InstantaneousBeatsPerMinute bpm="61" time="8:00:00.00"/>
                </HeartRateVariabilityMetadataList>
              </Record>
            </HealthData>"#,
        );
        assert_eq!(stats.records_read, 1, "children must not count as records");
        assert_eq!(stats.records_curated, 1);
        assert_eq!(rows.len(), 1);
        assert!(close(rows[0].value, 61.0));
    }

    #[test]
    fn non_record_elements_are_ignored() {
        let (rows, stats) = parse(
            r#"<HealthData locale="en_US">
              <ExportDate value="2026-07-31 09:00:00 -0700"/>
              <Me HKCharacteristicTypeIdentifierBiologicalSex="HKBiologicalSexMale"/>
              <Workout workoutActivityType="HKWorkoutActivityTypeRunning" duration="32"
                       startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:32:00 -0700"/>
              <ActivitySummary dateComponents="2026-07-28" activeEnergyBurned="512"/>
              <Correlation type="HKCorrelationTypeIdentifierBloodPressure"
                           startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700">
                <Record type="HKQuantityTypeIdentifierBloodPressureSystolic" unit="mmHg"
                        startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="118"/>
              </Correlation>
            </HealthData>"#,
        );
        assert!(rows.is_empty());
        assert_eq!(
            stats.records_read, 1,
            "only the nested Record counts; Workout and friends do not"
        );
        assert!(
            stats
                .quarantined
                .contains_key("HKQuantityTypeIdentifierBloodPressureSystolic")
        );
    }

    /// Apple's export opens with a declaration and a DOCTYPE carrying a large
    /// internal DTD subset full of `>` characters.
    #[test]
    fn xml_declaration_and_doctype_do_not_derail_the_parse() {
        let (rows, _) = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE HealthData [
              <!ELEMENT HealthData (ExportDate,Me,(Record|Correlation|Workout)*)>
              <!ATTLIST HealthData locale CDATA #REQUIRED>
              <!ELEMENT Record (MetadataEntry*)>
            ]>
            <HealthData locale="en_US">
              <Record type="HKQuantityTypeIdentifierBodyMass" unit="lb"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="200"/>
            </HealthData>"#,
        );
        assert_eq!(rows.len(), 1);
        assert!(close(rows[0].value, 200.0));
    }

    #[test]
    fn escaped_attribute_values_are_unescaped() {
        let (rows, _) = parse(
            r#"<HealthData>
              <Record type="HKQuantityTypeIdentifierBodyMass" sourceName="Mom &amp; Dad&apos;s Scale" unit="lb"
                      startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="200"/>
            </HealthData>"#,
        );
        assert_eq!(rows[0].source.as_deref(), Some("Mom & Dad's Scale"));
    }

    /// The multi-GB property, tested by its mechanism: quick-xml appends into
    /// the caller's buffer, so without a clear per iteration the buffer grows
    /// to the size of the file.
    #[test]
    fn event_buffer_does_not_grow_with_the_file() {
        use std::fmt::Write as _;

        let mut xml = String::from("<HealthData>\n");
        for i in 0..10_000 {
            let _ = writeln!(
                xml,
                r#"<Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Apple Watch" unit="count/min" startDate="2026-07-28 08:00:00 -0700" endDate="2026-07-28 08:00:00 -0700" value="{}"/>"#,
                60 + i % 40
            );
        }
        xml.push_str("</HealthData>\n");
        assert!(
            xml.len() > 1_000_000,
            "fixture must be big enough to expose growth, was {}",
            xml.len()
        );

        let mut buf = Vec::new();
        let mut acc = Accumulator::default();
        let mut stats = ImportStats::default();
        parse_into_buf(xml.as_bytes(), &mut buf, &mut acc, &mut stats).expect("parses");

        assert_eq!(stats.records_read, 10_000);
        assert!(
            buf.capacity() <= 4_096,
            "event buffer reached {} bytes for a {}-byte file — it is not being cleared",
            buf.capacity(),
            xml.len()
        );
    }

    #[test]
    fn malformed_xml_reports_its_byte_offset() {
        let mut acc = Accumulator::default();
        let mut stats = ImportStats::default();
        let err = parse_into(
            "<HealthData><Record unclosed=".as_bytes(),
            &mut acc,
            &mut stats,
        )
        .expect_err("malformed XML must fail");
        let msg = err.to_string();
        assert!(msg.contains("export.xml: byte"), "got {msg}");
    }
}
