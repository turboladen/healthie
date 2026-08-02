//! Rendering for the `import-apple-health` CLI report.
//!
//! Presentation only — every number here is computed in
//! `healthie_shared::services::apple_health` (ADR-0002: business logic never
//! lives in the backend). What this module decides is what the operator is
//! *shown*, and that matters more than usual for this command: the import is a
//! one-time, irreversible-in-practice backfill whose two riskiest assumptions
//! (the scale of Apple's percent-typed units, and the sleep-day boundary) can
//! only be checked by looking at what it actually wrote.

use std::{fmt::Write as _, path::Path};

use healthie_shared::services::apple_health::{
    ExistingData, ImportReport, KindReport, SleepDayShift, SleepDayVerdict,
};

/// Warn, before anything is read or written, that an import into a store that
/// already holds data is irreversible.
///
/// `None` when the store is empty: a first import has nothing to lose, and a
/// warning that fires on every run is one nobody reads — the same reasoning
/// that made the sleep day-shift check refuse to cry wolf.
///
/// This is the *pre*-flight half of the safety story. The report already
/// mentions backups, but only after the write, and post-hoc advice is not a
/// safeguard. Note that plain re-importing is destructive too: last-write-wins
/// overwrites every colliding `(kind, date)`, including rows the live HAE push
/// wrote, with or without `--replace-range`.
#[must_use]
pub fn preflight_warning(
    existing: &ExistingData,
    db_path: &str,
    replace_range: bool,
) -> Option<String> {
    if existing.is_empty() {
        return None;
    }
    let span = existing.date_range.map_or_else(
        || "no dated rows".to_owned(),
        |(from, to)| format!("{from} .. {to}"),
    );
    let mut out = String::new();
    let _ = writeln!(
        out,
        "⚠  This database already holds {} daily_metric rows ({span}).",
        existing.rows
    );
    // Line by line so rustfmt cannot rewrap it differently between runs.
    for line in [
        "   An import OVERWRITES any row it produces for the same (kind, date) — including",
        "   rows the live HAE push wrote. That is not reversible.",
    ] {
        let _ = writeln!(out, "{line}");
    }
    if replace_range {
        for line in [
            "   --replace-range is set, so this run will ALSO DELETE pre-existing rows in the",
            "   imported range that it does not itself produce. Both are irreversible.",
        ] {
            let _ = writeln!(out, "{line}");
        }
    }
    let _ = writeln!(out, "   Back up {db_path} before continuing.");
    let _ = writeln!(
        out,
        "   The export has not been opened and nothing has been written — Ctrl-C to abort.\n"
    );
    Some(out)
}

/// Format a finished import for stdout.
#[must_use]
pub fn render(report: &ImportReport, path: &Path) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Imported {}", path.display());
    let _ = writeln!(
        out,
        "  records read       {:>10}   curated {}  excluded {}  skipped {}",
        report.records_read,
        report.records_curated,
        report.records_excluded,
        report.records_skipped
    );
    let range = report.date_range.map_or_else(
        || "no dated rows".to_owned(),
        |(from, to)| format!("{from} .. {to}"),
    );
    let _ = writeln!(
        out,
        "  daily_metric rows  {:>10}   {range}",
        report.rows_written
    );
    if report.rows_overwritten > 0 {
        let _ = writeln!(
            out,
            "  overwrote          {:>10}   rows that already existed (last-write-wins)",
            report.rows_overwritten
        );
    }
    if report.stale_quarantine_cleared > 0 {
        let _ = writeln!(
            out,
            "  cleared            {:>10}   stale quarantine rows for now-handled names",
            report.stale_quarantine_cleared
        );
    }

    if !report.document_closed {
        // Worth saying even when nothing destructive was asked for: the counts
        // above describe a partial file, and look entirely ordinary.
        for line in [
            "",
            "  ⚠  THE EXPORT ENDED WITH ITS ROOT ELEMENT STILL OPEN — IT IS TRUNCATED.",
            "     An interrupted transfer of a multi-gigabyte export cut at a record boundary",
            "     parses without error and simply stops early. Everything it held was imported,",
            "     but treat the counts above as a lower bound and re-transfer the file.",
        ] {
            let _ = writeln!(out, "{line}");
        }
    }

    render_quarantine(&mut out, report);
    render_unconvertible(&mut out, report);
    render_kinds(&mut out, report);
    render_sum_sources(&mut out, report);
    render_sleep_shift(&mut out, report);
    render_stale_rows(&mut out, report);
    out
}

/// Rows the run did not rewrite. Loud, because nothing else in the report
/// distinguishes a stale row from a good one — the values look normal.
fn render_stale_rows(out: &mut String, report: &ImportReport) {
    if report.stale_rows.is_empty() {
        return;
    }
    let total: usize = report.stale_rows.iter().map(|s| s.count).sum();
    // Line by line so rustfmt cannot rewrap these differently on successive runs.
    let preamble: &[&str] = if report.stale_rows_deleted > 0 {
        &[
            "     These rows are GONE. If any of them were live-push data rather than an",
            "     earlier import's misplacement, restore them from your backup — nothing",
            "     here recorded which they were. Deleted, by kind:",
        ]
    } else if report.replace_range_refused_truncated {
        &[
            "     --replace-range was REFUSED: the export ended with its root element still",
            "     open, so it is truncated. Rows missing from a partial file are mostly rows",
            "     the file does not reach, not rows an earlier import misplaced. Re-transfer",
            "     the export and try again. Nothing was deleted.",
        ]
    } else {
        &[
            "     Either an earlier import placed them with a different sleep-day boundary or",
            "     metric mapping — in which case they hold known-wrong values that upsert alone",
            "     can never remove — OR they are days this export does not cover: the live HAE",
            "     push landed them, every reading of that kind hit an unconvertible unit, or",
            "     you deleted them in the Health app. Nothing records which.",
            "     --replace-range DELETES them. Check some of the dates below before using it.",
        ]
    };
    let heading = if report.stale_rows_deleted > 0 {
        format!(
            "\n  ⚠  DELETED {} PRE-EXISTING ROWS THIS RUN DID NOT REWRITE",
            report.stale_rows_deleted
        )
    } else {
        format!("\n  ⚠  {total} PRE-EXISTING ROWS IN THE IMPORTED RANGE WERE NOT REWRITTEN")
    };
    let _ = writeln!(out, "{heading}");
    for line in preamble {
        let _ = writeln!(out, "{line}");
    }
    // Enumerated in every case, deletion included: an irreversible removal of
    // health data must leave more of a record than a bare count.
    for stale in &report.stale_rows {
        let dates: Vec<String> = stale
            .sample_dates
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let ellipsis = if stale.count > stale.sample_dates.len() {
            ", …"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "       {:<24} {:>6}   e.g. {}{ellipsis}",
            format!("{:?}", stale.kind),
            stale.count,
            dates.join(", ")
        );
    }
}

fn render_quarantine(out: &mut String, report: &ImportReport) {
    if report.quarantined.is_empty() {
        return;
    }
    let total: u64 = report.quarantined.iter().map(|q| q.records_seen).sum();
    let _ = writeln!(
        out,
        "\n  quarantined  {} names / {total} records (kept in quarantined_metric)",
        report.quarantined.len()
    );
    // Loudest first — the high-volume names are the ones worth curating next.
    let mut names: Vec<_> = report.quarantined.iter().collect();
    names.sort_by_key(|q| std::cmp::Reverse(q.records_seen));
    for q in names {
        let _ = writeln!(out, "    {:<58} {:>9}", q.raw_name, q.records_seen);
    }
}

fn render_unconvertible(out: &mut String, report: &ImportReport) {
    if report.unconvertible.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\n  unconvertible units  {} (values NOT stored — never coerced)",
        report.unconvertible.len()
    );
    for u in &report.unconvertible {
        let _ = writeln!(
            out,
            "    {:<48} unit={:<8} {:>7}",
            u.raw_name, u.unit, u.records
        );
    }
}

fn render_kinds(out: &mut String, report: &ImportReport) {
    if report.per_kind.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\n  per kind (check the value span — it is how a unit-scale mistake shows itself)"
    );
    for k in &report.per_kind {
        let _ = write!(
            out,
            "    {:<24} {:>6} days   {:>12.3} .. {:<12.3} {}",
            format!("{:?}", k.kind),
            k.days,
            k.value_min,
            k.value_max,
            k.unit
        );
        if let Some(o) = k.overlap {
            let _ = write!(
                out,
                "   [overwrote {} days, mean Δ {:.3}, max Δ {:.3} on {}]",
                o.days, o.mean_abs_diff, o.max_abs_diff, o.max_diff_date
            );
        }
        if looks_like_a_fraction(k) {
            let _ = write!(out, "   ⚠ LOOKS LIKE A 0-1 FRACTION, NOT A PERCENT");
        }
        out.push('\n');
    }
    if report.per_kind.iter().any(looks_like_a_fraction) {
        // Written line by line: rustfmt rewraps a single long literal
        // differently on successive runs, so fmt-check never settles.
        for line in [
            "     ⚠  HealthKit stores percentages as a 0-1 fraction, so a span entirely at or",
            "        below 1.0 most likely needs a x100 conversion adding in units.rs. Verify",
            "        against the Health app before trusting these rows (bead healthie-4u7).",
        ] {
            let _ = writeln!(out, "{line}");
        }
    }
}

/// Whether a percent-typed kind's observed span suggests it is still a 0-1
/// fraction rather than a 0-100 percentage.
///
/// `units.rs` now scales Apple's percent fractions on the way in, so this
/// should never fire on `export.xml` data — it stays as a tripwire against a
/// future source that does not, and its silence is now itself a signal.
///
/// `value_max > 0.0` keeps an all-zero kind from tripping it: a column of
/// zeros is unscalable either way, and flagging it would be noise of exactly
/// the kind this report cannot afford.
fn looks_like_a_fraction(kind: &KindReport) -> bool {
    kind.unit == "%" && kind.days > 0 && kind.value_max > 0.0 && kind.value_max <= 1.0
}

fn render_sum_sources(out: &mut String, report: &ImportReport) {
    if report.sum_sources.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\n  multi-source days (totals take the largest single source, so these are a LOWER BOUND)"
    );
    for s in &report.sum_sources {
        let _ = writeln!(
            out,
            "    {:<24} {:>6} days   summed/kept: mean {:.2}x  worst {:.2}x",
            format!("{:?}", s.kind),
            s.days_multi_source,
            s.mean_ratio,
            s.worst_ratio
        );
    }
}

fn render_sleep_shift(out: &mut String, report: &ImportReport) {
    let (compared_days, prev_day, same_day, next_day) = match report.sleep_day_shift {
        SleepDayShift::NoComparableRows => {
            let _ = writeln!(
                out,
                "\n  sleep day boundary   no comparable sleep-total rows — NOT verified by this \
                 run"
            );
            return;
        }
        SleepDayShift::SelfComparison { compared_days } => {
            // Saying "agrees" here would retire the one question this import
            // cannot otherwise answer, on the strength of reading itself back.
            for line in [
                format!(
                    "\n  sleep day boundary   values are bit-identical across {compared_days} \
                     days, so this is almost"
                ),
                "                       certainly this import's own earlier output — NOT \
                 independently"
                    .to_owned(),
                "                       verified. Only the first import into a store holding live"
                    .to_owned(),
                "                       HAE rows can check this.".to_owned(),
            ] {
                let _ = writeln!(out, "{line}");
            }
            return;
        }
        SleepDayShift::Compared {
            compared_days,
            prev_day,
            same_day,
            next_day,
        } => (compared_days, prev_day, same_day, next_day),
    };
    let _ = writeln!(
        out,
        "\n  sleep day boundary   mean |Δ| vs existing rows over {compared_days} days:  D-1 \
         {prev_day:>6.2}   D {same_day:>6.2}   D+1 {next_day:>6.2}"
    );
    match report.sleep_day_shift.verdict() {
        SleepDayVerdict::Agrees => {
            let _ = writeln!(
                out,
                "                       no better fit one day either side — the boundary agrees \
                 with the existing rows."
            );
        }
        SleepDayVerdict::Mismatch { offset } => {
            let (direction, relation) = if offset < 0 {
                ("LATER", "D-1")
            } else {
                ("EARLIER", "D+1")
            };
            let _ = writeln!(
                out,
                "    ⚠  MISMATCH: imported nights line up with existing rows at {relation}, so \
                 backfilled sleep sits one day {direction} than the live rows.\n       Fix \
                 SLEEP_DAY_BOUNDARY_HOUR in \
                 healthie-shared/src/services/apple_health/accumulate.rs, then re-run (the import \
                 is idempotent)."
            );
        }
        SleepDayVerdict::Unverified => {}
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use healthie_shared::{
        entities::daily_metric::MetricKind,
        services::apple_health::{
            ExistingData, ImportReport, KindReport, Overlap, QuarantinedName, SleepDayShift,
            StaleRows, SumSourceReport, UnconvertibleUnit,
        },
        test_support::date,
    };

    use super::{preflight_warning, render};

    fn base() -> ImportReport {
        ImportReport {
            records_read: 100,
            records_curated: 90,
            records_excluded: 5,
            records_skipped: 5,
            rows_written: 12,
            rows_overwritten: 0,
            stale_quarantine_cleared: 0,
            date_range: Some((date("2016-01-01"), date("2026-07-30"))),
            quarantined: Vec::new(),
            unconvertible: Vec::new(),
            per_kind: Vec::new(),
            sum_sources: Vec::new(),
            sleep_day_shift: SleepDayShift::NoComparableRows,
            stale_rows: Vec::new(),
            stale_rows_deleted: 0,
            document_closed: true,
            replace_range_refused_truncated: false,
        }
    }

    fn populated() -> ExistingData {
        ExistingData {
            rows: 50_877,
            date_range: Some((date("2011-02-17"), date("2026-07-31"))),
        }
    }

    /// The warning has to reach the operator while aborting is still possible,
    /// and has to name the file they are being told to copy.
    #[test]
    fn preflight_warns_on_a_populated_database_and_names_the_path() {
        let out = preflight_warning(&populated(), "/srv/healthie/data/healthie.db", false)
            .expect("a populated store must warn");
        assert!(out.contains("50877") || out.contains("50,877"), "{out}");
        assert!(out.contains("2011-02-17 .. 2026-07-31"), "{out}");
        assert!(out.contains("/srv/healthie/data/healthie.db"), "{out}");
        assert!(out.contains("OVERWRITES"), "{out}");
        assert!(out.contains("not reversible"), "{out}");
        assert!(out.contains("Ctrl-C"), "{out}");
    }

    /// A first import into a fresh store has nothing to lose, and a warning
    /// that fires every run is one nobody reads.
    #[test]
    fn preflight_is_silent_on_an_empty_database() {
        let empty = ExistingData {
            rows: 0,
            date_range: None,
        };
        assert!(preflight_warning(&empty, "data/healthie.db", false).is_none());
        assert!(
            preflight_warning(&empty, "data/healthie.db", true).is_none(),
            "even with --replace-range there is nothing to destroy"
        );
    }

    /// A plain re-import overwrites; --replace-range overwrites AND deletes.
    /// The warning must distinguish them.
    #[test]
    fn preflight_escalates_for_replace_range() {
        let plain = preflight_warning(&populated(), "data/healthie.db", false).unwrap();
        assert!(!plain.contains("DELETE"), "{plain}");

        let replacing = preflight_warning(&populated(), "data/healthie.db", true).unwrap();
        assert!(replacing.contains("ALSO DELETE"), "{replacing}");
        assert!(replacing.contains("--replace-range is set"), "{replacing}");
        // The overwrite warning survives the escalation rather than being
        // replaced by it — that run does both.
        assert!(replacing.contains("OVERWRITES"), "{replacing}");
    }

    #[test]
    fn renders_counts_and_date_range() {
        let out = render(&base(), Path::new("/tmp/export.xml"));
        assert!(out.contains("/tmp/export.xml"));
        assert!(out.contains("2016-01-01 .. 2026-07-30"));
        assert!(out.contains("curated 90"));
    }

    /// A fresh database cannot validate the boundary, and the output must say
    /// so rather than implying agreement by silence.
    #[test]
    fn fresh_database_says_the_boundary_is_unverified() {
        let out = render(&base(), Path::new("export.xml"));
        assert!(out.contains("NOT verified by this run"), "{out}");
    }

    #[test]
    fn day_shift_mismatch_is_loud_and_names_the_fix() {
        let mut report = base();
        report.sleep_day_shift = SleepDayShift::Compared {
            compared_days: 400,
            prev_day: 0.11,
            same_day: 4.82,
            next_day: 5.03,
        };
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("MISMATCH"), "{out}");
        assert!(out.contains("one day LATER"), "{out}");
        assert!(out.contains("SLEEP_DAY_BOUNDARY_HOUR"), "{out}");
    }

    #[test]
    fn agreeing_boundary_is_reported_as_such() {
        let mut report = base();
        report.sleep_day_shift = SleepDayShift::Compared {
            compared_days: 400,
            prev_day: 4.9,
            same_day: 0.08,
            next_day: 5.1,
        };
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("the boundary agrees"), "{out}");
        assert!(!out.contains("MISMATCH"), "{out}");
    }

    /// Re-importing unchanged data makes every offset fit equally well. That is
    /// no evidence of a shift, and must not tell the operator to go change a
    /// constant and re-import a decade of history.
    #[test]
    fn identical_fits_do_not_raise_a_mismatch() {
        let mut report = base();
        report.sleep_day_shift = SleepDayShift::Compared {
            compared_days: 400,
            prev_day: 0.0,
            same_day: 0.0,
            next_day: 0.0,
        };
        let out = render(&report, Path::new("export.xml"));
        assert!(!out.contains("MISMATCH"), "a tie is not a mismatch:\n{out}");
    }

    /// Nor must ordinary noise, where a neighbor happens to edge ahead.
    #[test]
    fn marginally_better_neighbor_does_not_raise_a_mismatch() {
        let mut report = base();
        report.sleep_day_shift = SleepDayShift::Compared {
            compared_days: 400,
            prev_day: 0.30,
            same_day: 0.34,
            next_day: 4.9,
        };
        let out = render(&report, Path::new("export.xml"));
        assert!(!out.contains("MISMATCH"), "noise is not a mismatch:\n{out}");
    }

    /// The value span is the tripwire for a unit-scale mistake, so it has to be
    /// visible and labeled.
    #[test]
    fn per_kind_span_and_overlap_are_shown() {
        let mut report = base();
        report.rows_overwritten = 3;
        report.per_kind = vec![KindReport {
            kind: MetricKind::Spo2,
            unit: "%",
            days: 900,
            value_min: 0.91,
            value_max: 0.99,
            overlap: Some(Overlap {
                days: 3,
                mean_abs_diff: 1.5,
                max_abs_diff: 2.25,
                max_diff_date: date("2026-07-28"),
            }),
        }];
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("Spo2"), "{out}");
        assert!(out.contains("0.910"), "{out}");
        assert!(out.contains("overwrote 3 days"), "{out}");
        assert!(out.contains("2026-07-28"), "{out}");
    }

    /// Reading your own output back is not verification, and the wording must
    /// not let it read as any.
    #[test]
    fn self_comparison_is_reported_as_unverified_not_agreement() {
        let mut report = base();
        report.sleep_day_shift = SleepDayShift::SelfComparison {
            compared_days: 4_000,
        };
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("own earlier output"), "{out}");
        assert!(out.contains("NOT independently"), "{out}");
        // Hedged, not asserted: bit-identity is overwhelming evidence, not proof.
        assert!(out.contains("almost"), "{out}");
        assert!(!out.contains("agrees"), "{out}");
        assert!(!out.contains("MISMATCH"), "{out}");
    }

    /// Stale rows look entirely normal in the data, so the report is the only
    /// place they can surface — and it has to name the remedy.
    #[test]
    fn stale_rows_are_loud_and_name_the_remedy() {
        let mut report = base();
        report.stale_rows = vec![StaleRows {
            kind: MetricKind::SleepTotal,
            count: 312,
            sample_dates: vec![date("2019-04-02"), date("2019-06-11")],
        }];
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("NOT REWRITTEN"), "{out}");
        assert!(out.contains("--replace-range"), "{out}");
        assert!(out.contains("312"), "{out}");
        assert!(out.contains("2019-04-02"), "{out}");
    }

    /// An irreversible deletion of health data must leave more of a record than
    /// a bare count — which kinds, and which dates, so the operator can find
    /// them in a backup.
    #[test]
    fn deleted_stale_rows_keep_their_per_kind_detail() {
        let mut report = base();
        report.stale_rows = vec![StaleRows {
            kind: MetricKind::SleepTotal,
            count: 312,
            sample_dates: vec![date("2019-04-02"), date("2019-06-11")],
        }];
        report.stale_rows_deleted = 312;
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("DELETED 312"), "{out}");
        assert!(out.contains("SleepTotal"), "kinds must survive:\n{out}");
        assert!(out.contains("2019-04-02"), "dates must survive:\n{out}");
        assert!(out.contains("backup"), "{out}");
    }

    /// Deleting on the strength of a truncated file would remove rows the file
    /// merely never reached.
    #[test]
    fn refused_replace_range_says_why_and_confirms_nothing_was_deleted() {
        let mut report = base();
        report.document_closed = false;
        report.replace_range_refused_truncated = true;
        report.stale_rows = vec![StaleRows {
            kind: MetricKind::SleepTotal,
            count: 4,
            sample_dates: vec![date("2019-04-02")],
        }];
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("REFUSED"), "{out}");
        assert!(out.contains("truncated"), "{out}");
        assert!(out.contains("Nothing was deleted"), "{out}");
    }

    /// Truncation is worth saying even when nothing destructive was asked for:
    /// the counts describe a partial file and look entirely ordinary.
    #[test]
    fn truncated_export_is_reported_even_without_replace_range() {
        let mut report = base();
        report.document_closed = false;
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("TRUNCATED"), "{out}");
        assert!(out.contains("lower bound"), "{out}");
    }

    /// `HealthKit` stores percentages as a 0-1 fraction, so this is the
    /// expected case rather than a remote one — and `GaitAsymmetry`'s real
    /// 0-5 % range makes it easy to skim past unaided.
    #[test]
    fn fraction_scaled_percentages_are_flagged() {
        let mut report = base();
        report.per_kind = vec![
            KindReport {
                kind: MetricKind::Spo2,
                unit: "%",
                days: 900,
                value_min: 0.91,
                value_max: 0.99,
                overlap: None,
            },
            KindReport {
                kind: MetricKind::HeartRate,
                unit: "count/min",
                days: 900,
                value_min: 0.5,
                value_max: 0.9,
                overlap: None,
            },
        ];
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("LOOKS LIKE A 0-1 FRACTION"), "{out}");
        assert!(out.contains("healthie-4u7"), "{out}");
        // Only percent-typed kinds are candidates; a low count/min is just low.
        assert_eq!(
            out.matches("LOOKS LIKE A 0-1 FRACTION").count(),
            1,
            "only the %-unit kind should be flagged:\n{out}"
        );
    }

    /// After the units.rs scaling fix this is the normal case, so the flag
    /// must go quiet — otherwise it becomes the false alarm this report has
    /// already had to fix once.
    #[test]
    fn percentages_already_in_0_100_are_not_flagged() {
        let mut report = base();
        // The four %-typed kinds at their real post-scaling spans, from Steve's
        // 7.6M-record export.
        report.per_kind = vec![
            (MetricKind::BodyFat, 0.0, 30.3),
            (MetricKind::Spo2, 0.0, 98.5),
            (MetricKind::GaitAsymmetry, 0.0, 90.0),
            (MetricKind::GaitDoubleSupport, 25.9, 35.8),
        ]
        .into_iter()
        .map(|(kind, value_min, value_max)| KindReport {
            kind,
            unit: "%",
            days: 900,
            value_min,
            value_max,
            overlap: None,
        })
        .collect();
        let out = render(&report, Path::new("export.xml"));
        assert!(
            !out.contains("LOOKS LIKE A 0-1 FRACTION"),
            "correctly-scaled percentages must not flag:\n{out}"
        );
        assert!(!out.contains("healthie-4u7"), "{out}");
    }

    /// A column of zeros cannot be misread in scale in any detectable way, and
    /// flagging it would be pure noise.
    #[test]
    fn an_all_zero_percent_kind_is_not_flagged() {
        let mut report = base();
        report.per_kind = vec![KindReport {
            kind: MetricKind::GaitAsymmetry,
            unit: "%",
            days: 900,
            value_min: 0.0,
            value_max: 0.0,
            overlap: None,
        }];
        let out = render(&report, Path::new("export.xml"));
        assert!(!out.contains("LOOKS LIKE A 0-1 FRACTION"), "{out}");
    }

    #[test]
    fn quarantine_is_listed_loudest_first() {
        let mut report = base();
        report.quarantined = vec![
            QuarantinedName {
                raw_name: "HKQuantityTypeIdentifierSmall".to_owned(),
                records_seen: 3,
            },
            QuarantinedName {
                raw_name: "HKQuantityTypeIdentifierHuge".to_owned(),
                records_seen: 90_000,
            },
        ];
        let out = render(&report, Path::new("export.xml"));
        let huge = out.find("Huge").expect("huge listed");
        let small = out.find("Small").expect("small listed");
        assert!(
            huge < small,
            "highest-volume name should come first:\n{out}"
        );
    }

    #[test]
    fn unconvertible_units_and_lower_bound_warning_are_shown() {
        let mut report = base();
        report.unconvertible = vec![UnconvertibleUnit {
            raw_name: "HKQuantityTypeIdentifierBodyMass".to_owned(),
            unit: "mmHg".to_owned(),
            records: 3,
        }];
        report.sum_sources = vec![SumSourceReport {
            kind: MetricKind::Steps,
            days_multi_source: 120,
            mean_ratio: 1.42,
            worst_ratio: 1.98,
        }];
        let out = render(&report, Path::new("export.xml"));
        assert!(out.contains("NOT stored"), "{out}");
        assert!(out.contains("mmHg"), "{out}");
        assert!(out.contains("LOWER BOUND"), "{out}");
        assert!(out.contains("1.98x"), "{out}");
    }
}
