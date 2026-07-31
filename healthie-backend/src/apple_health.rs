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

use healthie_shared::services::apple_health::{ImportReport, SleepDayShift, SleepDayVerdict};

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

    render_quarantine(&mut out, report);
    render_unconvertible(&mut out, report);
    render_kinds(&mut out, report);
    render_sum_sources(&mut out, report);
    render_sleep_shift(&mut out, report);
    out
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
        out.push('\n');
    }
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
    let SleepDayShift::Compared {
        compared_days,
        prev_day,
        same_day,
        next_day,
    } = report.sleep_day_shift
    else {
        let _ = writeln!(
            out,
            "\n  sleep day boundary   no existing sleep-total rows to compare against — NOT \
             verified by this run"
        );
        return;
    };
    let show = |v: Option<f64>| v.map_or_else(|| "     -".to_owned(), |d| format!("{d:>6.2}"));
    let _ = writeln!(
        out,
        "\n  sleep day boundary   mean |Δ| vs existing rows over {compared_days} days:  D-1 {}   \
         D {}   D+1 {}",
        show(prev_day),
        show(same_day),
        show(next_day)
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
            ImportReport, KindReport, Overlap, QuarantinedName, SleepDayShift, SumSourceReport,
            UnconvertibleUnit,
        },
        test_support::date,
    };

    use super::render;

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
        }
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
            prev_day: Some(0.11),
            same_day: Some(4.82),
            next_day: Some(5.03),
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
            prev_day: Some(4.9),
            same_day: Some(0.08),
            next_day: Some(5.1),
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
            prev_day: Some(0.0),
            same_day: Some(0.0),
            next_day: Some(0.0),
        };
        let out = render(&report, Path::new("export.xml"));
        assert!(!out.contains("MISMATCH"), "a tie is not a mismatch:\n{out}");
    }

    /// Nor must ordinary noise, where a neighbour happens to edge ahead.
    #[test]
    fn marginally_better_neighbour_does_not_raise_a_mismatch() {
        let mut report = base();
        report.sleep_day_shift = SleepDayShift::Compared {
            compared_days: 400,
            prev_day: Some(0.30),
            same_day: Some(0.34),
            next_day: Some(4.9),
        };
        let out = render(&report, Path::new("export.xml"));
        assert!(!out.contains("MISMATCH"), "noise is not a mismatch:\n{out}");
    }

    /// The value span is the tripwire for a unit-scale mistake, so it has to be
    /// visible and labelled.
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
