//! Per-kind bounds on what a reading can physically be, for both intakes.
//!
//! # These encode impossibility, not unusualness
//!
//! A value outside these bounds is **not a measurement**: it is a sensor
//! artifact, a stuck recorder, or a producer bug. A value inside them is stored
//! untouched however strange it looks, because deciding what is *unusual* needs
//! the real distribution — and that is healthie-4lf.2's job, over exactly the
//! data these bounds must not distort. So the ceilings are deliberately
//! generous and the interesting limits are definitional: 0% oxygen saturation,
//! 0 lb, 0 bpm.
//!
//! Nothing here clamps. Clamping manufactures a measurement — a 49.9 h night
//! rounded to 24 h is a number every later reader would trust, and we do not
//! know how long he slept. The honest row is no row.
//!
//! # Why the bounds are per kind rather than one rule
//!
//! Because a single rule is provably wrong on this data. Steve's imported
//! export holds 243 `gait-asymmetry` days and 2 `breathing-disturbances` days
//! at exactly 0.0 — a perfectly symmetric gait and a night with no disturbances
//! are what a *good* reading looks like — beside 40 `body-fat` and 23 `spo2`
//! days at 0.0, which are impossible. A blanket "positive values only" would
//! destroy 245 real rows to catch 63 artifacts. Zero is a legitimate reading
//! for some kinds and impossible for others, and the unit does not say which.
//!
//! # What they deliberately do NOT catch
//!
//! A **scale error**. An unscaled 0-1 percentage (`0.985` blood oxygen) sits
//! comfortably inside `(0, 100]` and always will. ADR-0006 §6 settled that a
//! range heuristic gets this wrong — `GaitDoubleSupport` arrives spanning
//! `0.259 .. 0.358`, entirely plausible *as* a percentage — so the value-span
//! report remains the tripwire for scale, and these bounds neither replace nor
//! weaken it.

use crate::entities::{daily_metric::MetricKind, quarantined_metric::QuarantineReason};

/// Whether zero is a reading or an impossibility for a given kind.
///
/// An enum rather than a `bool` field because the call site reads as the claim
/// being made: `Above(0.0)` says a zero cannot happen, `AtLeast(0.0)` says it
/// can and means something.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LowerBound {
    /// `value >= n` — `n` itself is a real reading.
    AtLeast(f64),
    /// `value > n` — `n` is not survivable, not achievable, or not a number
    /// this metric can take.
    Above(f64),
}

/// What one kind's readings can physically be. The upper bound is inclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub(crate) lower: LowerBound,
    pub(crate) upper: f64,
}

impl Bounds {
    fn admits(self, value: f64) -> bool {
        let above_floor = match self.lower {
            LowerBound::AtLeast(n) => value >= n,
            LowerBound::Above(n) => value > n,
        };
        above_floor && value <= self.upper
    }
}

/// The physical limits of one metric, in [`MetricKind::unit`]'s unit.
///
/// The match is exhaustive with no wildcard on purpose: a new [`MetricKind`]
/// fails to compile here until someone decides what it can physically be.
// Arms that happen to share numbers are kept apart: each carries the reasoning
// for *that* metric, and merging them by value would collapse independent
// judgments into an anonymous list.
#[allow(clippy::match_same_arms)]
pub(crate) fn bounds(kind: MetricKind) -> Bounds {
    match kind {
        // A body cannot weigh nothing. The ceiling clears the heaviest weight
        // ever recorded for a human by a wide margin.
        MetricKind::Weight => above(0.0, 1500.0),
        // 0% body fat is not survivable — 40 days of it in the real export are
        // sensor artifacts. 100% is the definitional ceiling.
        MetricKind::BodyFat => above(0.0, 100.0),
        // The world-record VO2 max is ~96; 150 leaves room for a bad estimate
        // without admitting a decimal-point slip.
        MetricKind::Vo2Max => above(0.0, 150.0),
        // Asystole is not a measurement. The observed real maximum is 208.
        MetricKind::RestingHeartRate | MetricKind::HeartRate => above(0.0, 300.0),
        // NOT a rate despite its `count/min` unit: this is the number of beats
        // the heart dropped in the minute after exercise. A zero-beat recovery
        // is a poor result, not an impossible reading — so unlike every other
        // `count/min` kind, zero is admitted.
        MetricKind::CardioRecovery => at_least(0.0, 300.0),
        // A heart with no beat-to-beat variation at all is not a living one.
        MetricKind::Hrv => above(0.0, 1000.0),
        // 0% blood oxygen is not survivable; 23 days of it in the real export
        // are a sensor artifact landing in the `min` column, which is the
        // single defect that made this module P1.
        MetricKind::Spo2 => above(0.0, 100.0),
        // Zero disturbances is a good night, not a missing reading.
        MetricKind::BreathingDisturbances => at_least(0.0, 10_000.0),
        MetricKind::RespiratoryRate => above(0.0, 200.0),
        MetricKind::BloodPressureSystolic => above(0.0, 400.0),
        MetricKind::BloodPressureDiastolic => above(0.0, 300.0),
        // A day in bed burns no *active* energy. The ceiling is well past a
        // Tour de France stage.
        MetricKind::ActiveEnergy => at_least(0.0, 20_000.0),
        MetricKind::Steps => at_least(0.0, 200_000.0),
        // A local day holds 1440 minutes — except on a DST fall-back day, which
        // holds 1500.
        MetricKind::ExerciseMinutes | MetricKind::StandMinutes => at_least(0.0, 1500.0),
        MetricKind::WalkingDistance => at_least(0.0, 350.0),
        MetricKind::FlightsClimbed => at_least(0.0, 10_000.0),
        // A speed of zero is not a walking speed; the ceiling clears the
        // fastest human sprint (~28 mph).
        MetricKind::WalkingSpeed => above(0.0, 30.0),
        // A perfectly symmetric gait reads 0.0, and 243 real days do. This is
        // the pair a blanket positive-only rule would have destroyed.
        MetricKind::GaitAsymmetry | MetricKind::GaitDoubleSupport => at_least(0.0, 100.0),
        MetricKind::StepLength => above(0.0, 100.0),
        // Sleep is folded into a sleep-day, and no one sleeps more than a day
        // of it. 25 rather than 24 because a DST fall-back day genuinely holds
        // 25 wall-clock hours, and 24 would refuse a legitimate longest day.
        //
        // This is a physiological judgment, NOT a property of the fold. Sleep
        // segments are attributed by their start hour and never split
        // (`accumulate::sleep_date`), so a single 36-hour segment lands whole
        // on one date — which is exactly how the real export produced a 49.9 h
        // night. Nothing about the construction bounds this above.
        MetricKind::SleepTotal
        | MetricKind::SleepDeep
        | MetricKind::SleepRem
        | MetricKind::SleepCore
        | MetricKind::SleepAwake => at_least(0.0, 25.0),
        // Time in bed is not a claim about consciousness, and more than a day
        // continuously in bed is ordinary for a bedridden or post-operative
        // day. Past two days, a stuck recorder is the better explanation —
        // which is what the real export's 52.9 h was.
        MetricKind::TimeInBed => at_least(0.0, 48.0),
    }
}

fn above(floor: f64, upper: f64) -> Bounds {
    Bounds {
        lower: LowerBound::Above(floor),
        upper,
    }
}

fn at_least(floor: f64, upper: f64) -> Bounds {
    Bounds {
        lower: LowerBound::AtLeast(floor),
        upper,
    }
}

/// `None` when `value` is storable for `kind`; otherwise why it is not.
///
/// Non-finiteness is reported separately from being out of range so the report
/// can tell a producer bug from a sensor artifact — and because a NaN fails
/// every comparison, which would otherwise make it indistinguishable from a
/// number that is merely too large.
pub(crate) fn reject_reason(kind: MetricKind, value: f64) -> Option<QuarantineReason> {
    if !value.is_finite() {
        return Some(QuarantineReason::NonFiniteValue);
    }
    (!bounds(kind).admits(value)).then_some(QuarantineReason::ImplausibleValue)
}

#[cfg(test)]
mod tests {
    use sea_orm::strum::IntoEnumIterator as _;

    use super::{LowerBound, bounds, reject_reason};
    use crate::entities::{daily_metric::MetricKind, quarantined_metric::QuarantineReason};

    /// A transposed or typo'd table entry would silently refuse an entire
    /// metric, which is the one failure mode this table can have that nothing
    /// else would notice.
    #[test]
    fn every_kind_has_ordered_finite_bounds() {
        for kind in MetricKind::iter() {
            let b = bounds(kind);
            let floor = match b.lower {
                LowerBound::AtLeast(n) | LowerBound::Above(n) => n,
            };
            assert!(floor.is_finite(), "{kind:?}: lower bound must be finite");
            assert!(b.upper.is_finite(), "{kind:?}: upper bound must be finite");
            assert!(
                floor < b.upper,
                "{kind:?}: lower {floor} must be below upper {}",
                b.upper
            );
        }
    }

    /// The regression test for the risk this whole change carries: that
    /// tightening the intakes starts refusing data that legitimately lands.
    ///
    /// Every value below was **measured** from Steve's real 15-year import
    /// (50,877 rows), not invented — these are the observed extremes per kind.
    /// A future tightening has to argue with real data to get past this.
    #[test]
    fn observed_real_world_values_are_all_plausible() {
        for (kind, value) in [
            (MetricKind::Weight, 216.397),
            (MetricKind::Weight, 246.808),
            (MetricKind::BodyFat, 30.295),
            (MetricKind::Vo2Max, 32.48),
            (MetricKind::Vo2Max, 46.742),
            (MetricKind::RestingHeartRate, 42.0),
            (MetricKind::HeartRate, 29.0),
            (MetricKind::HeartRate, 208.0),
            (MetricKind::Hrv, 15.236),
            (MetricKind::Hrv, 255.012),
            (MetricKind::Spo2, 100.0),
            (MetricKind::BreathingDisturbances, 37.49),
            (MetricKind::RespiratoryRate, 7.0),
            (MetricKind::RespiratoryRate, 35.5),
            (MetricKind::CardioRecovery, 20.483),
            (MetricKind::BloodPressureSystolic, 134.0),
            (MetricKind::BloodPressureDiastolic, 62.0),
            (MetricKind::ActiveEnergy, 2.572),
            (MetricKind::ActiveEnergy, 1627.557),
            (MetricKind::Steps, 7.0),
            (MetricKind::Steps, 29606.0),
            (MetricKind::ExerciseMinutes, 197.0),
            (MetricKind::StandMinutes, 536.0),
            (MetricKind::WalkingDistance, 0.004),
            (MetricKind::WalkingDistance, 13.343),
            (MetricKind::FlightsClimbed, 207.0),
            (MetricKind::WalkingSpeed, 1.23),
            (MetricKind::WalkingSpeed, 3.042),
            (MetricKind::GaitAsymmetry, 90.0),
            (MetricKind::GaitDoubleSupport, 35.8),
            (MetricKind::StepLength, 16.142),
            (MetricKind::StepLength, 30.46),
            // The longest night that is a real night. The 49.877 h one is not.
            (MetricKind::SleepTotal, 14.34),
            (MetricKind::SleepAwake, 6.925),
            (MetricKind::SleepCore, 9.304),
            (MetricKind::TimeInBed, 0.1),
        ] {
            assert_eq!(
                reject_reason(kind, value),
                None,
                "{kind:?} = {value} was observed in real data and must store"
            );
        }
    }

    /// Zero is a reading for some kinds and impossible for others. A blanket
    /// rule in either direction destroys real data: on the measured import,
    /// 245 legitimate zero rows on one side and 63 artifacts on the other.
    #[test]
    fn zero_is_impossible_only_where_it_is() {
        for kind in [
            MetricKind::Weight,
            MetricKind::BodyFat,
            MetricKind::Spo2,
            MetricKind::HeartRate,
            MetricKind::RestingHeartRate,
            MetricKind::Hrv,
            MetricKind::Vo2Max,
            MetricKind::RespiratoryRate,
            MetricKind::WalkingSpeed,
            MetricKind::StepLength,
            MetricKind::BloodPressureSystolic,
            MetricKind::BloodPressureDiastolic,
        ] {
            assert_eq!(
                reject_reason(kind, 0.0),
                Some(QuarantineReason::ImplausibleValue),
                "{kind:?} cannot physically read zero"
            );
        }
        for kind in [
            // 243 real days of a perfectly symmetric gait.
            MetricKind::GaitAsymmetry,
            MetricKind::GaitDoubleSupport,
            // 2 real days with no disturbances at all.
            MetricKind::BreathingDisturbances,
            // A delta, not a rate: no recovery is a bad result, not an
            // impossible reading.
            MetricKind::CardioRecovery,
            MetricKind::Steps,
            MetricKind::ActiveEnergy,
            MetricKind::ExerciseMinutes,
            MetricKind::StandMinutes,
            MetricKind::WalkingDistance,
            MetricKind::FlightsClimbed,
            MetricKind::SleepDeep,
            MetricKind::SleepAwake,
        ] {
            assert_eq!(
                reject_reason(kind, 0.0),
                None,
                "{kind:?} reading zero is a measurement, not an artifact"
            );
        }
    }

    /// The confirmed outliers from the real import, and the ordinary readings
    /// either side of them that must survive.
    #[test]
    fn the_real_outliers_are_refused_and_their_neighbors_are_not() {
        assert_eq!(
            reject_reason(MetricKind::SleepTotal, 49.877),
            Some(QuarantineReason::ImplausibleValue),
            "2023-12-29: a sleep app left running for a day and a half"
        );
        assert_eq!(
            reject_reason(MetricKind::TimeInBed, 52.953),
            Some(QuarantineReason::ImplausibleValue)
        );
        assert_eq!(reject_reason(MetricKind::SleepTotal, 14.34), None);
        // 25 h, not 24: a DST fall-back day really does hold that much.
        assert_eq!(reject_reason(MetricKind::SleepTotal, 24.5), None);
        assert_eq!(
            reject_reason(MetricKind::Spo2, 0.0),
            Some(QuarantineReason::ImplausibleValue)
        );
        // A genuine desaturation emergency is a reading, not an artifact, and
        // must never be the thing this table throws away.
        assert_eq!(reject_reason(MetricKind::Spo2, 60.0), None);
        assert_eq!(
            reject_reason(MetricKind::Spo2, 101.0),
            Some(QuarantineReason::ImplausibleValue)
        );
    }

    /// Non-finite is reported as itself. A NaN fails every comparison, so
    /// without the explicit check it would be indistinguishable from a number
    /// that is merely out of range — and the two mean different things about
    /// where the problem is.
    #[test]
    fn non_finite_is_rejected_under_its_own_reason() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                reject_reason(MetricKind::Weight, value),
                Some(QuarantineReason::NonFiniteValue)
            );
        }
    }
}
