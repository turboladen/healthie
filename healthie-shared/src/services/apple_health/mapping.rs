//! Apple Health `export.xml` type-identifier vocabulary: the curated /
//! excluded / quarantined trichotomy of ADR-0005 §4, expressed over
//! `HKQuantityTypeIdentifier*` and `HKCategoryTypeIdentifier*` names.
//!
//! The backfill and the live HAE push are two intakes onto the same
//! `daily_metric` store, so a curated `export.xml` name MUST resolve to the
//! same [`MetricKind`] as its HAE counterpart — otherwise the same physical
//! reading lands on two different kinds depending on how it arrived. That is
//! enforced structurally: [`HK_METRICS`] is a single table carrying *both*
//! spellings alongside the kind, and `hk_and_hae_names_agree` walks it against
//! [`map_hae_name`](crate::services::metrics::map_hae_name). There is no way to
//! add one spelling without the other.
//!
//! A misspelled identifier here is benign-and-loud rather than silently wrong:
//! an unrecognized name falls through to `Unknown` and quarantines, so it
//! surfaces in the import report as an uncurated name rather than landing data
//! on the wrong kind. Every string below was verified against Apple's
//! `HKQuantityTypeIdentifier` reference and against real `export.xml` files
//! (note the acronym casing: `VO2Max`, `SDNN`).

use crate::entities::daily_metric::MetricKind;

/// The `type` attribute of a sleep record; its stage lives in `value`.
pub(crate) const HK_SLEEP_ANALYSIS: &str = "HKCategoryTypeIdentifierSleepAnalysis";

/// How an `export.xml` `type` attribute is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HkMapping {
    /// A quantity name → one [`MetricKind`], aggregated per local day.
    Curated(MetricKind),
    /// `HKCategoryTypeIdentifierSleepAnalysis` → timed segments, folded per
    /// stage per night on a separate path.
    Sleep,
    /// Seen and deliberately not tracked — silently ignored, which is what
    /// keeps quarantine exceptional (ADR-0005 §4).
    Excluded,
    /// Never mapped → quarantined, never dropped.
    Unknown,
}

/// The curated vocabulary, carrying the `export.xml` spelling, the HAE spelling
/// and the shared [`MetricKind`] in one row so the two intake paths cannot
/// drift apart. Adding a metric means adding one row here and one arm to
/// `map_hae_name`; the agreement test fails until both exist.
pub(crate) const HK_METRICS: &[(&str, &str, MetricKind)] = &[
    (
        "HKQuantityTypeIdentifierBodyMass",
        "weight_body_mass",
        MetricKind::Weight,
    ),
    (
        "HKQuantityTypeIdentifierBodyFatPercentage",
        "body_fat_percentage",
        MetricKind::BodyFat,
    ),
    (
        "HKQuantityTypeIdentifierVO2Max",
        "vo2_max",
        MetricKind::Vo2Max,
    ),
    (
        "HKQuantityTypeIdentifierRestingHeartRate",
        "resting_heart_rate",
        MetricKind::RestingHeartRate,
    ),
    (
        "HKQuantityTypeIdentifierHeartRate",
        "heart_rate",
        MetricKind::HeartRate,
    ),
    (
        "HKQuantityTypeIdentifierHeartRateVariabilitySDNN",
        "heart_rate_variability",
        MetricKind::Hrv,
    ),
    (
        "HKQuantityTypeIdentifierOxygenSaturation",
        "blood_oxygen_saturation",
        MetricKind::Spo2,
    ),
    (
        "HKQuantityTypeIdentifierAppleSleepingBreathingDisturbances",
        "breathing_disturbances",
        MetricKind::BreathingDisturbances,
    ),
    (
        "HKQuantityTypeIdentifierRespiratoryRate",
        "respiratory_rate",
        MetricKind::RespiratoryRate,
    ),
    (
        "HKQuantityTypeIdentifierHeartRateRecoveryOneMinute",
        "cardio_recovery",
        MetricKind::CardioRecovery,
    ),
    (
        "HKQuantityTypeIdentifierActiveEnergyBurned",
        "active_energy",
        MetricKind::ActiveEnergy,
    ),
    (
        "HKQuantityTypeIdentifierStepCount",
        "step_count",
        MetricKind::Steps,
    ),
    (
        "HKQuantityTypeIdentifierAppleExerciseTime",
        "apple_exercise_time",
        MetricKind::ExerciseMinutes,
    ),
    (
        "HKQuantityTypeIdentifierDistanceWalkingRunning",
        "walking_running_distance",
        MetricKind::WalkingDistance,
    ),
    (
        "HKQuantityTypeIdentifierAppleStandTime",
        "apple_stand_time",
        MetricKind::StandMinutes,
    ),
    (
        "HKQuantityTypeIdentifierWalkingSpeed",
        "walking_speed",
        MetricKind::WalkingSpeed,
    ),
    (
        "HKQuantityTypeIdentifierWalkingAsymmetryPercentage",
        "walking_asymmetry_percentage",
        MetricKind::GaitAsymmetry,
    ),
    (
        "HKQuantityTypeIdentifierWalkingDoubleSupportPercentage",
        "walking_double_support_percentage",
        MetricKind::GaitDoubleSupport,
    ),
    (
        "HKQuantityTypeIdentifierWalkingStepLength",
        "walking_step_length",
        MetricKind::StepLength,
    ),
];

/// `export.xml` names we have seen and deliberately do not curate — the mirror
/// of [`EXCLUDED_HAE_NAMES`](crate::services::metrics::EXCLUDED_HAE_NAMES).
///
/// This list is what keeps quarantine *exceptional* on the backfill path:
/// `BasalEnergyBurned` alone contributes hundreds of thousands of records to a
/// decade of history, and without an explicit decline it would dominate the
/// import report and bury genuinely new names.
pub(crate) const EXCLUDED_HK_NAMES: &[&str] = &[
    "HKCategoryTypeIdentifierAppleStandHour",
    "HKQuantityTypeIdentifierBasalEnergyBurned",
    "HKQuantityTypeIdentifierPhysicalEffort",
    "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
    "HKQuantityTypeIdentifierTimeInDaylight",
    "HKQuantityTypeIdentifierWalkingHeartRateAverage",
    "HKQuantityTypeIdentifierEnvironmentalAudioExposure",
    "HKQuantityTypeIdentifierHeadphoneAudioExposure",
];

/// Classify an `export.xml` `type` attribute into curated / sleep / excluded /
/// unknown.
pub(crate) fn map_hk_name(name: &str) -> HkMapping {
    if name == HK_SLEEP_ANALYSIS {
        return HkMapping::Sleep;
    }
    if let Some((_, _, kind)) = HK_METRICS.iter().find(|(hk, _, _)| *hk == name) {
        return HkMapping::Curated(*kind);
    }
    if EXCLUDED_HK_NAMES.contains(&name) {
        return HkMapping::Excluded;
    }
    HkMapping::Unknown
}

/// Every `export.xml` name this path recognizes — curated, sleep, or
/// deliberately excluded. Used to sweep stale quarantine rows once a name is
/// promoted (a re-run must not leave the old row advertising it as unhandled).
pub(crate) fn is_recognized_hk_name(name: &str) -> bool {
    !matches!(map_hk_name(name), HkMapping::Unknown)
}

/// Which sleep sub-metrics one `HKCategoryValueSleepAnalysis*` segment feeds.
///
/// Apple emits no "total sleep" segment, so [`MetricKind::SleepTotal`] is
/// *derived*: every asleep-class stage also contributes to a combined asleep
/// interval set, whose union becomes the night's total. `Awake` and `InBed` are
/// deliberately excluded from that union — they are not sleep.
///
/// Two undifferentiated spellings exist and both must be handled: pre-iOS-16
/// exports carry `…Asleep`, iOS 16+ carries `…AsleepUnspecified`. Neither has a
/// stage breakdown, so they feed the total alone — a decade of history has
/// years of each. Returns `(stage_kind, counts_as_asleep)`.
pub(crate) fn map_sleep_stage(value: &str) -> Option<(Option<MetricKind>, bool)> {
    match value {
        "HKCategoryValueSleepAnalysisInBed" => Some((Some(MetricKind::TimeInBed), false)),
        "HKCategoryValueSleepAnalysisAwake" => Some((Some(MetricKind::SleepAwake), false)),
        "HKCategoryValueSleepAnalysisAsleepCore" => Some((Some(MetricKind::SleepCore), true)),
        "HKCategoryValueSleepAnalysisAsleepDeep" => Some((Some(MetricKind::SleepDeep), true)),
        "HKCategoryValueSleepAnalysisAsleepREM" => Some((Some(MetricKind::SleepRem), true)),
        // Undifferentiated: contributes to the total, but has no stage row.
        "HKCategoryValueSleepAnalysisAsleep" | "HKCategoryValueSleepAnalysisAsleepUnspecified" => {
            Some((None, true))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::strum::IntoEnumIterator as _;

    use super::{
        EXCLUDED_HK_NAMES, HK_METRICS, HK_SLEEP_ANALYSIS, HkMapping, is_recognized_hk_name,
        map_hk_name, map_sleep_stage,
    };
    use crate::{
        entities::daily_metric::MetricKind,
        services::metrics::{EXCLUDED_HAE_NAMES, HaeMapping, map_hae_name},
    };

    /// The whole point of the shared table: a reading that arrives by backfill
    /// and the same reading arriving by live HAE push must land on one kind.
    #[test]
    fn hk_and_hae_names_agree() {
        for (hk, hae, kind) in HK_METRICS {
            assert_eq!(
                map_hk_name(hk),
                HkMapping::Curated(*kind),
                "{hk} must map to {kind:?}"
            );
            assert!(
                matches!(map_hae_name(hae), HaeMapping::Curated(k) if k == *kind),
                "HAE name {hae} disagrees with export.xml name {hk} (expected {kind:?})"
            );
        }
    }

    /// A future `MetricKind` must not be silently unreachable from the
    /// backfill: every non-sleep kind needs exactly one export.xml spelling.
    #[test]
    fn every_non_sleep_kind_is_mapped_exactly_once() {
        for kind in MetricKind::iter() {
            let hits = HK_METRICS.iter().filter(|(_, _, k)| *k == kind).count();
            let expected = usize::from(!is_sleep_kind(kind));
            assert_eq!(
                hits, expected,
                "{kind:?} appears {hits} times in HK_METRICS, expected {expected}"
            );
        }
    }

    fn is_sleep_kind(kind: MetricKind) -> bool {
        matches!(
            kind,
            MetricKind::SleepTotal
                | MetricKind::SleepDeep
                | MetricKind::SleepRem
                | MetricKind::SleepCore
                | MetricKind::SleepAwake
                | MetricKind::TimeInBed
        )
    }

    #[test]
    fn excluded_hk_mirrors_excluded_hae() {
        assert_eq!(
            EXCLUDED_HK_NAMES.len(),
            EXCLUDED_HAE_NAMES.len(),
            "the two exclude lists must stay in lockstep"
        );
        for name in EXCLUDED_HK_NAMES {
            assert_eq!(map_hk_name(name), HkMapping::Excluded, "{name}");
        }
    }

    #[test]
    fn sleep_and_unknown_classify() {
        assert_eq!(map_hk_name(HK_SLEEP_ANALYSIS), HkMapping::Sleep);
        assert_eq!(
            map_hk_name("HKQuantityTypeIdentifierDietaryWater"),
            HkMapping::Unknown
        );
        assert!(is_recognized_hk_name("HKQuantityTypeIdentifierBodyMass"));
        assert!(is_recognized_hk_name(HK_SLEEP_ANALYSIS));
        assert!(is_recognized_hk_name(
            "HKQuantityTypeIdentifierBasalEnergyBurned"
        ));
        assert!(!is_recognized_hk_name(
            "HKQuantityTypeIdentifierDietaryWater"
        ));
    }

    /// Both the modern stage spellings and the pre-iOS-16 undifferentiated one
    /// must resolve — a decade of history contains years of each.
    #[test]
    fn sleep_stages_map_including_legacy_spellings() {
        assert_eq!(
            map_sleep_stage("HKCategoryValueSleepAnalysisAsleepDeep"),
            Some((Some(MetricKind::SleepDeep), true))
        );
        assert_eq!(
            map_sleep_stage("HKCategoryValueSleepAnalysisAsleepREM"),
            Some((Some(MetricKind::SleepRem), true))
        );
        assert_eq!(
            map_sleep_stage("HKCategoryValueSleepAnalysisInBed"),
            Some((Some(MetricKind::TimeInBed), false)),
            "in-bed is not sleep and must not feed the total"
        );
        assert_eq!(
            map_sleep_stage("HKCategoryValueSleepAnalysisAwake"),
            Some((Some(MetricKind::SleepAwake), false)),
            "awake is not sleep and must not feed the total"
        );
        for legacy in [
            "HKCategoryValueSleepAnalysisAsleep",
            "HKCategoryValueSleepAnalysisAsleepUnspecified",
        ] {
            assert_eq!(
                map_sleep_stage(legacy),
                Some((None, true)),
                "{legacy} counts toward the total with no stage row"
            );
        }
        assert_eq!(map_sleep_stage("HKCategoryValueSleepAnalysisFuture"), None);
    }
}
