//! Conversion of a declared unit to the canonical one [`MetricKind::unit`]
//! declares, for **both** intakes.
//!
//! Every producer stamps its readings with the unit they were recorded in,
//! which varies by device locale and by app: the same `BodyMass` history can
//! carry `kg` for years and `lb` after a phone was reconfigured. `daily_metric`
//! has no unit column — the unit is derived from the kind — so a value must be
//! converted before it is stored, and a value that *cannot* be converted must
//! never be stored (ADR-0006 §5, extended to the live path by ADR-0007).
//!
//! This lives beside the two intakes rather than inside either. It arrived with
//! the `export.xml` backfill and read as that importer's private helper, but
//! the vocabulary below always described **two** vendors' spellings, and the
//! live HAE path needs exactly the same refusal.
//!
//! # Where the numbers come from
//!
//! The arithmetic is [UCUM](https://ucum.org)'s, via `ucum-units`, whose
//! factors are generated at build time from the standard's own machine-readable
//! `ucum-essence.xml`. Nothing here transcribes a physical constant by hand.
//!
//! That mattered more than it sounds. Two other units crates were measured
//! against the hand-derived table this replaced and **both were less
//! accurate**: one truncates the international pound to seven significant
//! figures, the other hardcodes 1609 m per mile in its speed module (while
//! getting it right in its length module) and uses the International Steam
//! Table calorie where food energy wants the thermochemical one. UCUM agrees
//! with the old table on all twenty conversions — thirteen bit-exact, seven
//! differing only in the last ULP.
//!
//! # What UCUM cannot do, and is not asked to
//!
//! **Vocabulary.** Apple and HAE write `mL/min·kg`, `lbs`, `mph`, `mmHg` — none
//! of which are UCUM codes (`mmHg` in particular does not validate; the code is
//! `mm[Hg]`). Mapping those spellings onto codes is [`to_ucum`], and it stays
//! ours because it describes two vendors' habits, not physics.
//!
//! **Percent scaling.** A `%` may mean 0-1 or 0-100 depending on who wrote it.
//! That is a *scale convention*, not a dimensional conversion — UCUM correctly
//! reports `%`→`%` as unity — so it is applied separately, and it belongs to
//! the [`Producer`] rather than to the unit string.

use crate::entities::daily_metric::MetricKind;

/// Which intake produced a reading.
///
/// Percent scale is a property of the **producer**, not of the unit string, and
/// conflating the two is a 100x error waiting to happen. `HealthKit`'s `%` is a
/// 0-1 fraction; nothing says another exporter agrees, and this codebase has
/// evidence about exactly one of them.
///
/// It is the only thing conversion needs to know about who is calling. Every
/// other difference between the two intakes — per-record versus pre-aggregated,
/// quarantine granularity — is handled by the callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Producer {
    /// The `export.xml` backfill (`services::apple_health`).
    AppleExportXml,
    /// The live Health Auto Export push (`services::metrics`).
    HealthAutoExport,
}

impl Producer {
    /// Multiplier taking this producer's percent convention to canonical 0-100.
    fn percent_scale(self) -> f64 {
        match self {
            // Apple writes `%` for `HealthKit`'s `HKUnit.percent()`, which is a
            // **0-1 fraction**: 30.3% body fat is exported as `0.303` and a
            // 98.5% blood-oxygen reading as `0.985`.
            //
            // Confirmed against a real 7.6M-record export (2026-07-31), not
            // inferred — and worth the wait, because a range heuristic would
            // have got it wrong: `GaitDoubleSupport` arrived spanning
            // `0.259 .. 0.358`, entirely plausible *as* a percentage, so
            // guessing would have left that one 100x low while correctly
            // fixing blood oxygen.
            Self::AppleExportXml => 100.0,
            // UNVERIFIED. HAE is a different producer and nothing in this
            // codebase records its convention — Steve's device was unreachable
            // when this was written. Unity is chosen because it preserves
            // exactly what the live path did before it converted at all, so
            // routing HAE through this function cannot silently move a stored
            // number by 100x in either direction.
            //
            // healthie-t58 settles it the way healthie-4u7 settled the export
            // side: by reading the observed span on the first real push. Until
            // then `ingest_hae` warns when a percent kind arrives at or below
            // 1.0, which is what a fraction-sending HAE would look like.
            Self::HealthAutoExport => 1.0,
        }
    }
}

/// The UCUM code for a percentage, in both vocabularies.
pub(crate) const UCUM_PERCENT: &str = "%";

/// Convert `value` from `raw_unit`, as written by `producer`, into `kind`'s
/// canonical unit.
///
/// Returns `None` when the unit is not in our vocabulary, or is in it but
/// measures the wrong thing — the caller quarantines rather than storing.
/// Unit strings are matched case-insensitively where that is safe, and
/// tolerate Apple's punctuation variants (`mL/min·kg` vs `ml/(kg·min)`).
pub(crate) fn convert_to_canonical(
    raw_unit: &str,
    kind: MetricKind,
    value: f64,
    producer: Producer,
) -> Option<f64> {
    let target = kind.unit();
    // Fast path for the overwhelming majority of records: byte-identical unit
    // strings that also agree on scale. Percent is excluded because it is the
    // one unit where the strings match but the scales need not. This also keeps
    // UCUM's expression parsing off the hot loop for records that already
    // arrive canonical — most of the 7.6M in a real export.
    if raw_unit == target && target != UCUM_PERCENT {
        return Some(value);
    }

    let from = to_ucum(raw_unit)?;
    // Derived from the same vocabulary map rather than a second table keyed on
    // `MetricKind`, so a canonical unit and its UCUM code cannot drift apart.
    let to = to_ucum(target)?;

    if from == UCUM_PERCENT && to == UCUM_PERCENT {
        return Some(value * producer.percent_scale());
    }
    // Second fast path, on the CODE rather than the spelling: `mL/min·kg` and
    // `ml/(kg·min)` are not byte-identical to the canonical string but resolve
    // to the same UCUM code, as does Apple's `Cal` against canonical `kcal`.
    // Those are high-volume records, and a conversion to itself is unity — so
    // short-circuit rather than parse two expressions per record to learn it.
    if from == to {
        return Some(value);
    }
    // `Err` covers both an incomparable dimension and a code UCUM cannot
    // parse; either way the caller must not store the value.
    ucum::convert(value, from, to).ok()
}

/// Map an Apple or HAE unit spelling onto its UCUM code.
///
/// This is the layer no units library can supply: it encodes how two specific
/// vendors happen to write units, including spellings UCUM rejects outright.
/// `None` means "not in our vocabulary", which quarantines.
fn to_ucum(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim();
    // Resolved before case folding: Apple writes `Cal` for the kilocalorie, but
    // a lowercase `cal` is conventionally the small calorie — a 1000x
    // difference. Folding first would merge them, so `Cal` is promoted here and
    // a bare lowercase `cal` falls through to `None` rather than being guessed.
    if trimmed == "Cal" {
        return Some("kcal_th");
    }
    let folded = fold(trimmed);
    Some(match folded.as_str() {
        // Dimensionless and count-like.
        "%" | "percent" => UCUM_PERCENT,
        "count" | "steps" => "{count}",
        "count/min" | "bpm" | "beats/min" => "/min",
        // Mass.
        "kg" | "kilogram" | "kilograms" => "kg",
        "g" | "gram" | "grams" => "g",
        "lb" | "lbs" | "pound" | "pounds" => "[lb_av]",
        "st" | "stone" | "stones" => "[stone_av]",
        "oz" | "ounce" | "ounces" => "[oz_av]",
        // Length.
        "km" => "km",
        "m" | "meter" | "meters" => "m",
        "cm" => "cm",
        "mm" => "mm",
        "mi" | "mile" | "miles" => "[mi_i]",
        "ft" | "foot" | "feet" => "[ft_i]",
        "yd" | "yard" | "yards" => "[yd_i]",
        "in" | "inch" | "inches" => "[in_i]",
        // Energy. Apple's `Cal` is handled above, before case folding.
        "kcal" | "kilocalorie" | "kilocalories" => "kcal_th",
        "kj" => "kJ",
        "j" | "joule" | "joules" => "J",
        // Velocity. `fold` normalizes case and separators but not the `hr`/`h`
        // suffix, so both spellings are listed rather than derived.
        "mi/hr" | "mi/h" | "mph" => "[mi_i]/h",
        "km/hr" | "km/h" | "kph" => "km/h",
        "m/s" | "m/sec" => "m/s",
        // Duration. The plural abbreviations are cheap insurance against an
        // expensive failure: an HAE upgrade writing `hrs` for `sleep_analysis`
        // would quarantine every sleep stage on every push, indefinitely, and
        // sleep is what the briefing leans on hardest.
        "ms" => "ms",
        "s" | "sec" | "secs" | "second" | "seconds" => "s",
        "min" | "mins" | "minute" | "minutes" => "min",
        "hr" | "hrs" | "h" | "hour" | "hours" => "h",
        // Pressure. Apple writes `mmHg`, which UCUM does not accept.
        "mmhg" => "mm[Hg]",
        // VO2 max: a compound Apple writes several ways.
        "ml/kg/min" | "ml/min/kg" => "mL/min/kg",
        _ => return None,
    })
}

/// Fold a unit string to a comparison token: case, whitespace, parentheses and
/// Apple's `·`/`*` product separators are all noise.
///
/// `ml/(kg·min)` and `mL/min·kg` both reduce to a slash-separated form, which
/// the match above maps onto one UCUM code.
fn fold(trimmed: &str) -> String {
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '·' | '*' => out.push('/'),
            '(' | ')' | ' ' | '\t' => {}
            c => out.extend(c.to_lowercase()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use sea_orm::strum::IntoEnumIterator as _;

    use super::{Producer, convert_to_canonical, to_ucum};
    use crate::entities::daily_metric::MetricKind;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Spellings a producer may reasonably write that the vocabulary did not
    /// accept. `fold` normalizes case, whitespace and Apple's `·`/`*` product
    /// separators — it does **not** normalize `hr` against `h`, nor know that
    /// `steps` and `beats` are counts, so each spelling has to be listed.
    ///
    /// An unlisted spelling is not a cosmetic gap: it quarantines every record
    /// of that metric, and on the live path that is a whole kind going missing
    /// from every push.
    #[test]
    fn plausible_spellings_are_all_in_the_vocabulary() {
        for (spelling, kind, expected) in [
            ("km/h", MetricKind::WalkingSpeed, 0.621_371_192_237_334),
            ("mi/h", MetricKind::WalkingSpeed, 1.0),
            ("beats/min", MetricKind::HeartRate, 1.0),
            ("steps", MetricKind::Steps, 1.0),
            // Plural abbreviations: an unlisted one would quarantine every
            // sleep stage on every push.
            ("hrs", MetricKind::SleepTotal, 1.0),
            ("mins", MetricKind::SleepTotal, 1.0 / 60.0),
            ("secs", MetricKind::SleepTotal, 1.0 / 3600.0),
        ] {
            let got = convert_to_canonical(spelling, kind, 1.0, Producer::HealthAutoExport)
                .unwrap_or_else(|| panic!("{spelling} must be in the vocabulary"));
            assert!(
                close(got, expected),
                "{spelling}: got {got}, want {expected}"
            );
        }
        // Not an oversight to be "fixed": a lowercase `cal` is conventionally
        // the small calorie, and guessing between it and Apple's `Cal` is a
        // 1000x error. It stays refused.
        assert_eq!(
            convert_to_canonical(
                "cal",
                MetricKind::ActiveEnergy,
                1.0,
                Producer::HealthAutoExport
            ),
            None
        );
    }

    #[test]
    fn identity_when_already_canonical() {
        assert!(close(
            convert_to_canonical("lb", MetricKind::Weight, 234.2, Producer::AppleExportXml)
                .unwrap(),
            234.2
        ));
        assert!(close(
            convert_to_canonical(
                "count/min",
                MetricKind::HeartRate,
                61.0,
                Producer::AppleExportXml
            )
            .unwrap(),
            61.0
        ));
        assert!(close(
            convert_to_canonical(
                "mmHg",
                MetricKind::BloodPressureSystolic,
                118.0,
                Producer::AppleExportXml
            )
            .unwrap(),
            118.0
        ));
    }

    /// Every kind's own canonical unit must round-trip, or that kind could
    /// never be imported at all. This also proves every canonical spelling is
    /// in the vocabulary map — the map supplies the *target* code, so a missing
    /// entry would silently make the whole kind unconvertible.
    #[test]
    fn every_kind_accepts_its_own_canonical_unit() {
        for kind in MetricKind::iter() {
            assert!(
                to_ucum(kind.unit()).is_some(),
                "{kind:?}'s canonical unit {} has no UCUM code",
                kind.unit()
            );
            assert!(
                convert_to_canonical(kind.unit(), kind, 1.0, Producer::AppleExportXml).is_some(),
                "{kind:?} rejects its own unit {}",
                kind.unit()
            );
        }
    }

    /// Every UCUM code the vocabulary maps to must actually be valid UCUM. A
    /// typo would quarantine an entire metric silently.
    #[test]
    fn every_mapped_code_is_valid_ucum() {
        for kind in MetricKind::iter() {
            let code = to_ucum(kind.unit()).expect("canonical unit maps");
            assert!(
                ucum::validate(code).is_ok(),
                "{kind:?}: {code} is not valid UCUM"
            );
        }
        for spelling in [
            "kg",
            "g",
            "lbs",
            "st",
            "oz",
            "km",
            "m",
            "cm",
            "mm",
            "mi",
            "ft",
            "yd",
            "in",
            "kcal",
            "Cal",
            "kJ",
            "J",
            "mph",
            "kph",
            "m/s",
            "ms",
            "s",
            "min",
            "hr",
            "mmHg",
            "mL/min·kg",
            "count",
            "count/min",
            "%",
        ] {
            let code = to_ucum(spelling).unwrap_or_else(|| panic!("{spelling} maps"));
            assert!(
                ucum::validate(code).is_ok(),
                "{spelling} -> {code} is not valid UCUM"
            );
        }
    }

    /// The values UCUM produces, against the hand-derived table it replaced.
    /// Those constants were each checked against their defining standards, so
    /// they remain the oracle rather than something to update to match.
    #[test]
    fn conversions_match_the_hand_derived_oracle() {
        let cases: &[(&str, MetricKind, f64)] = &[
            ("kg", MetricKind::Weight, 2.204_622_621_848_776),
            ("g", MetricKind::Weight, 0.002_204_622_621_848_776),
            ("st", MetricKind::Weight, 14.0),
            ("oz", MetricKind::Weight, 0.0625),
            ("km", MetricKind::WalkingDistance, 0.621_371_192_237_334),
            ("m", MetricKind::WalkingDistance, 0.000_621_371_192_237_334),
            (
                "ft",
                MetricKind::WalkingDistance,
                0.000_189_393_939_393_939_4,
            ),
            (
                "yd",
                MetricKind::WalkingDistance,
                0.000_568_181_818_181_818_2,
            ),
            ("kJ", MetricKind::ActiveEnergy, 0.239_005_736_137_667_3),
            ("J", MetricKind::ActiveEnergy, 0.000_239_005_736_137_667_3),
            ("km/hr", MetricKind::WalkingSpeed, 0.621_371_192_237_334),
            ("m/s", MetricKind::WalkingSpeed, 2.236_936_292_054_402),
            ("cm", MetricKind::StepLength, 0.393_700_787_401_574_8),
            ("mm", MetricKind::StepLength, 0.039_370_078_740_157_48),
            ("m", MetricKind::StepLength, 39.370_078_740_157_48),
            ("ft", MetricKind::StepLength, 12.0),
            ("s", MetricKind::ExerciseMinutes, 1.0 / 60.0),
            ("hr", MetricKind::ExerciseMinutes, 60.0),
            ("min", MetricKind::TimeInBed, 1.0 / 60.0),
            ("s", MetricKind::TimeInBed, 1.0 / 3600.0),
        ];
        for (from, kind, expected) in cases {
            let got = convert_to_canonical(from, *kind, 1.0, Producer::AppleExportXml)
                .unwrap_or_else(|| panic!("{from} -> {kind:?} must convert"));
            let rel = ((got - expected) / expected).abs();
            // 1e-12 admits last-ULP re-association (observed max 2e-16) while
            // still catching any genuine constant disagreement, which for the
            // two rejected crates ran from 6.6e-8 to 6.7e-4.
            assert!(
                rel < 1e-12,
                "{from} -> {kind:?}: got {got:.17}, oracle {expected:.17} (rel {rel:.2e})"
            );
        }
    }

    /// Apple writes `Cal` for the kilocalorie; a lowercase `cal` is ambiguous
    /// with the small calorie and must quarantine rather than be guessed at.
    #[test]
    fn apple_cal_is_kilocalories_but_lowercase_cal_is_refused() {
        assert!(close(
            convert_to_canonical(
                "Cal",
                MetricKind::ActiveEnergy,
                512.0,
                Producer::AppleExportXml
            )
            .unwrap(),
            512.0
        ));
        assert_eq!(
            convert_to_canonical(
                "cal",
                MetricKind::ActiveEnergy,
                512.0,
                Producer::AppleExportXml
            ),
            None,
            "lowercase cal is ambiguous — quarantine, never a 1000x guess"
        );
    }

    /// export.xml spells VO2 max differently from our canonical string; the two
    /// denote the same unit and must not be treated as a conversion failure.
    #[test]
    fn vo2_max_punctuation_variants_are_the_same_unit() {
        for spelling in ["mL/min·kg", "ml/(kg·min)", "mL/min*kg", "ML/MIN·KG"] {
            assert!(
                close(
                    convert_to_canonical(
                        spelling,
                        MetricKind::Vo2Max,
                        42.0,
                        Producer::AppleExportXml
                    )
                    .unwrap(),
                    42.0
                ),
                "{spelling} should be recognized as canonical"
            );
        }
    }

    /// The one unit where matching strings do NOT mean matching scales: Apple's
    /// `%` is a 0-1 fraction, ours is 0-100. UCUM reports `%`→`%` as unity, so
    /// this scaling is ours and stays outside the conversion.
    #[test]
    fn apple_percent_fractions_scale_to_0_100() {
        // Real spans from Steve's export: body fat 0.303, blood oxygen 0.985.
        assert!(close(
            convert_to_canonical("%", MetricKind::BodyFat, 0.303, Producer::AppleExportXml)
                .unwrap(),
            30.3
        ));
        assert!(close(
            convert_to_canonical("%", MetricKind::Spo2, 0.985, Producer::AppleExportXml).unwrap(),
            98.5
        ));
        // Including the one a range heuristic would have got wrong: 0.259 reads
        // as a plausible percentage on its own.
        assert!(close(
            convert_to_canonical(
                "%",
                MetricKind::GaitDoubleSupport,
                0.259,
                Producer::AppleExportXml
            )
            .unwrap(),
            25.9
        ));
        assert!(close(
            convert_to_canonical(
                "%",
                MetricKind::GaitAsymmetry,
                0.9,
                Producer::AppleExportXml
            )
            .unwrap(),
            90.0
        ));
    }

    /// The same `%` string, the same kind, two producers, two scales — which is
    /// the whole reason the scale is the producer's and not the unit's.
    ///
    /// HAE's convention is UNVERIFIED (healthie-t58). Unity is pinned here not
    /// because it is known correct but because it is what the live path did
    /// before it converted at all: this test is what fails, loudly and in one
    /// place, on the day t58 is answered.
    #[test]
    fn percent_scale_follows_the_producer_not_the_unit() {
        let apple = convert_to_canonical("%", MetricKind::BodyFat, 0.303, Producer::AppleExportXml);
        let hae = convert_to_canonical("%", MetricKind::BodyFat, 0.303, Producer::HealthAutoExport);
        assert!(close(apple.unwrap(), 30.3), "Apple writes a 0-1 fraction");
        assert!(
            close(hae.unwrap(), 0.303),
            "HAE is assumed already 0-100, so its value passes through untouched"
        );
    }

    /// Scaling must not leak into units that merely look similar.
    #[test]
    fn percent_scaling_is_confined_to_percent_kinds() {
        assert!(close(
            convert_to_canonical(
                "count/min",
                MetricKind::HeartRate,
                61.0,
                Producer::AppleExportXml
            )
            .unwrap(),
            61.0
        ));
        assert!(close(
            convert_to_canonical(
                "count",
                MetricKind::FlightsClimbed,
                12.0,
                Producer::AppleExportXml
            )
            .unwrap(),
            12.0
        ));
        assert!(close(
            convert_to_canonical(
                "mmHg",
                MetricKind::BloodPressureDiastolic,
                78.0,
                Producer::AppleExportXml
            )
            .unwrap(),
            78.0
        ));
    }

    /// Two ways to be unconvertible, both of which must refuse rather than
    /// coerce: a unit outside our vocabulary, and one inside it that measures
    /// the wrong thing.
    #[test]
    fn unknown_and_wrong_dimension_units_are_both_refused() {
        // Not in the vocabulary at all.
        assert_eq!(
            convert_to_canonical(
                "furlong",
                MetricKind::Weight,
                21.0,
                Producer::AppleExportXml
            ),
            None
        );
        assert_eq!(
            convert_to_canonical("", MetricKind::Steps, 1.0, Producer::AppleExportXml),
            None
        );
        // Known units, wrong dimension for the kind.
        assert_eq!(
            convert_to_canonical(
                "mmHg",
                MetricKind::HeartRate,
                120.0,
                Producer::AppleExportXml
            ),
            None
        );
        assert_eq!(
            convert_to_canonical(
                "kg",
                MetricKind::WalkingDistance,
                5.0,
                Producer::AppleExportXml
            ),
            None
        );
        assert_eq!(
            convert_to_canonical("min", MetricKind::Weight, 5.0, Producer::AppleExportXml),
            None
        );
    }

    /// Apple's `mmHg` is not valid UCUM — the code is `mm[Hg]`. Pinned because
    /// it is the clearest case of why the vocabulary layer cannot be delegated
    /// to the units library.
    #[test]
    fn apple_spellings_that_ucum_rejects_are_translated() {
        assert!(
            ucum::validate("mmHg").is_err(),
            "if UCUM ever accepts this, the translation below is redundant"
        );
        assert_eq!(to_ucum("mmHg"), Some("mm[Hg]"));
        assert!(ucum::validate("mm[Hg]").is_ok());
    }
}
