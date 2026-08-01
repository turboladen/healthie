//! Conversion of `export.xml`'s per-record `unit` attribute to the canonical
//! unit [`MetricKind::unit`] declares.
//!
//! Apple stamps every quantity record with the unit it was recorded in, which
//! varies by device locale and by app: the same `BodyMass` history can carry
//! `kg` for years and `lb` after a phone was reconfigured. `daily_metric` has
//! no unit column — the unit is derived from the kind — so a value must be
//! converted before it is stored, and a value that *cannot* be converted must
//! never be stored.
//!
//! [`convert_to_canonical`] therefore returns `Option`, not a lossy best
//! effort. `None` means the caller quarantines the record verbatim rather than
//! coercing it: silently writing `78` kg into a column that means pounds
//! produces a plausible number that no later reader can detect as wrong.

use crate::entities::daily_metric::MetricKind;

/// Apple writes `%` for `HealthKit`'s `HKUnit.percent()`, which is a **0-1
/// fraction**, not a 0-100 percentage: 30.3% body fat is exported as `0.303`
/// and a 98.5% blood-oxygen reading as `0.985`. Our canonical `%` is 0-100, so
/// the same unit string means a different scale on each side and every
/// percent-typed reading is scaled on the way in.
///
/// Confirmed against a real 7.6M-record export (2026-07-31), not inferred —
/// and worth the wait, because a range heuristic would have got it wrong:
/// `GaitDoubleSupport` arrived spanning `0.259 .. 0.358`, entirely plausible
/// *as* a percentage, so guessing would have left that one 100x low while
/// correctly fixing blood oxygen.
const PERCENT_SCALE: f64 = 100.0;

/// Convert `value` from `raw_unit` into `kind`'s canonical unit.
///
/// Returns `None` when no conversion is known — the caller must quarantine
/// rather than store. Unit strings are compared case-insensitively and
/// tolerate Apple's punctuation variants (`mL/min·kg` vs `ml/(kg·min)`).
pub(crate) fn convert_to_canonical(raw_unit: &str, kind: MetricKind, value: f64) -> Option<f64> {
    let target = kind.unit();
    // Fast path for the overwhelming majority of records: byte-identical unit
    // strings that also agree on scale. Percent is excluded because it is the
    // one unit where the strings match but the scales do not.
    if raw_unit == target && !is_percent(target) {
        return Some(value);
    }
    let from = normalize_unit(raw_unit);
    let to = normalize_unit(target);
    if from == to {
        return Some(if is_percent(&to) {
            value * PERCENT_SCALE
        } else {
            value
        });
    }
    factor(&from, &to).map(|f| value * f)
}

fn is_percent(unit: &str) -> bool {
    unit == "%"
}

/// Fold a unit string to a comparison token: case, whitespace, parentheses and
/// Apple's `·`/`*` product separators are all noise.
///
/// `Cal` is resolved *before* case folding on purpose. Apple writes `Cal` for
/// the kilocalorie, but a lowercase `cal` is conventionally the small calorie —
/// a 1000x difference. Case-folding first would merge them, so `Cal` is
/// promoted here and a bare lowercase `cal` is left with no factor, which
/// quarantines it instead of guessing.
fn normalize_unit(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed == "Cal" {
        return "kcal".to_owned();
    }
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '·' | '*' => out.push('/'),
            '(' | ')' | ' ' | '\t' => {}
            c => out.extend(c.to_lowercase()),
        }
    }
    alias(&out).to_owned()
}

/// Collapse spelling variants onto one token per physical unit.
fn alias(unit: &str) -> &str {
    match unit {
        "ml/min/kg" | "ml/kg/min" => "ml/kg/min",
        "lbs" | "pound" | "pounds" => "lb",
        "percent" => "%",
        "mph" => "mi/hr",
        "km/h" | "kph" => "km/hr",
        "m/sec" => "m/s",
        "inch" | "inches" => "in",
        "hour" | "hours" | "h" => "hr",
        "minute" | "minutes" => "min",
        "sec" | "second" | "seconds" => "s",
        "kilocalorie" | "kilocalories" => "kcal",
        "kilogram" | "kilograms" => "kg",
        other => other,
    }
}

/// Multiplicative factor taking a normalized `from` unit to a normalized `to`
/// unit, or `None` when the pair is not a known conversion.
// Some pairs share a factor by coincidence (km→mi and km/hr→mi/hr; s→min and
// min→hr). They are distinct conversions between distinct units and must stay
// separately readable, so the table is grouped by dimension rather than folded
// by value.
#[allow(clippy::match_same_arms)]
fn factor(from: &str, to: &str) -> Option<f64> {
    Some(match (from, to) {
        // mass → lb
        ("kg", "lb") => 2.204_622_621_848_776,
        ("g", "lb") => 0.002_204_622_621_848_776,
        ("st", "lb") => 14.0,
        ("oz", "lb") => 0.0625,
        // distance → mi
        ("km", "mi") => 0.621_371_192_237_334,
        ("m", "mi") => 0.000_621_371_192_237_334,
        ("ft", "mi") => 0.000_189_393_939_393_939_4,
        ("yd", "mi") => 0.000_568_181_818_181_818_2,
        // energy → kcal
        ("kj", "kcal") => 0.239_005_736_137_667_3,
        ("j", "kcal") => 0.000_239_005_736_137_667_3,
        // speed → mi/hr
        ("km/hr", "mi/hr") => 0.621_371_192_237_334,
        ("m/s", "mi/hr") => 2.236_936_292_054_402,
        // length → in
        ("cm", "in") => 0.393_700_787_401_574_8,
        ("mm", "in") => 0.039_370_078_740_157_48,
        ("m", "in") => 39.370_078_740_157_48,
        ("ft", "in") => 12.0,
        // duration → min
        ("s", "min") => 1.0 / 60.0,
        ("hr", "min") => 60.0,
        // duration → hr
        ("min", "hr") => 1.0 / 60.0,
        ("s", "hr") => 1.0 / 3600.0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use sea_orm::strum::IntoEnumIterator as _;

    use super::convert_to_canonical;
    use crate::entities::daily_metric::MetricKind;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn identity_when_already_canonical() {
        assert!(close(
            convert_to_canonical("lb", MetricKind::Weight, 234.2).unwrap(),
            234.2
        ));
        assert!(close(
            convert_to_canonical("count/min", MetricKind::HeartRate, 61.0).unwrap(),
            61.0
        ));
        assert!(close(
            convert_to_canonical("mmHg", MetricKind::BloodPressureSystolic, 118.0).unwrap(),
            118.0
        ));
    }

    /// The one unit where matching strings do NOT mean matching scales: Apple's
    /// `%` is a 0-1 fraction, ours is 0-100.
    #[test]
    fn apple_percent_fractions_scale_to_0_100() {
        // Real spans from Steve's export: body fat 0.303, blood oxygen 0.985.
        assert!(close(
            convert_to_canonical("%", MetricKind::BodyFat, 0.303).unwrap(),
            30.3
        ));
        assert!(close(
            convert_to_canonical("%", MetricKind::Spo2, 0.985).unwrap(),
            98.5
        ));
        // Including the one a range heuristic would have got wrong: 0.259
        // reads as a plausible percentage on its own.
        assert!(close(
            convert_to_canonical("%", MetricKind::GaitDoubleSupport, 0.259).unwrap(),
            25.9
        ));
        assert!(close(
            convert_to_canonical("%", MetricKind::GaitAsymmetry, 0.9).unwrap(),
            90.0
        ));
    }

    /// Scaling must not leak into units that merely look similar.
    #[test]
    fn percent_scaling_is_confined_to_percent_kinds() {
        assert!(close(
            convert_to_canonical("count/min", MetricKind::HeartRate, 61.0).unwrap(),
            61.0
        ));
        assert!(close(
            convert_to_canonical("count", MetricKind::FlightsClimbed, 12.0).unwrap(),
            12.0
        ));
        assert!(close(
            convert_to_canonical("mmHg", MetricKind::BloodPressureDiastolic, 78.0).unwrap(),
            78.0
        ));
    }

    /// Every kind's own canonical unit must round-trip, or that kind could
    /// never be imported at all.
    #[test]
    fn every_kind_accepts_its_own_canonical_unit() {
        for kind in MetricKind::iter() {
            assert!(
                convert_to_canonical(kind.unit(), kind, 1.0).is_some(),
                "{kind:?} rejects its own unit {}",
                kind.unit()
            );
        }
    }

    #[test]
    fn mass_distance_energy_speed_length_convert() {
        assert!(close(
            convert_to_canonical("kg", MetricKind::Weight, 100.0).unwrap(),
            220.462_262_184_877_6
        ));
        assert!(close(
            convert_to_canonical("km", MetricKind::WalkingDistance, 5.0).unwrap(),
            3.106_855_961_186_67
        ));
        assert!(close(
            convert_to_canonical("m/s", MetricKind::WalkingSpeed, 1.0).unwrap(),
            2.236_936_292_054_402
        ));
        assert!(close(
            convert_to_canonical("cm", MetricKind::StepLength, 76.2).unwrap(),
            30.0
        ));
        assert!(close(
            convert_to_canonical("kJ", MetricKind::ActiveEnergy, 1000.0).unwrap(),
            239.005_736_137_667_3
        ));
    }

    /// Apple writes `Cal` for the kilocalorie; a lowercase `cal` is ambiguous
    /// with the small calorie and must quarantine rather than be guessed at.
    #[test]
    fn apple_cal_is_kilocalories_but_lowercase_cal_is_refused() {
        assert!(close(
            convert_to_canonical("Cal", MetricKind::ActiveEnergy, 512.0).unwrap(),
            512.0
        ));
        assert_eq!(
            convert_to_canonical("cal", MetricKind::ActiveEnergy, 512.0),
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
                    convert_to_canonical(spelling, MetricKind::Vo2Max, 42.0).unwrap(),
                    42.0
                ),
                "{spelling} should be recognized as canonical"
            );
        }
    }

    #[test]
    fn unknown_unit_is_refused_not_coerced() {
        assert_eq!(convert_to_canonical("degC", MetricKind::Weight, 21.0), None);
        assert_eq!(convert_to_canonical("", MetricKind::Steps, 1.0), None);
        assert_eq!(
            convert_to_canonical("mmHg", MetricKind::HeartRate, 120.0),
            None
        );
    }
}
