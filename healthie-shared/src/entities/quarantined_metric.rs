//! Verbatim data points an intake would not store (ADR-0002 "never silently
//! dropped", ADR-0005 §4, ADR-0007). Upserted per `(raw_name, date)` — the
//! durable record of everything refused, and the discovery surface for metrics
//! Apple/HAE add later.
//!
//! Two kinds of row live here, told apart by `raw_point._import.reason`:
//!
//! - A name that is **neither curated nor explicitly excluded**. This is the
//!   discovery case ADR-0005 §4 describes.
//! - A **curated** name whose point could not be stored anyway — its unit was
//!   missing or unconvertible, or its value was not physically possible
//!   (ADR-0007 §1, §3). The name is understood; this specific reading is not
//!   trustworthy, and refusing it silently would be the coercion ADR-0006 §6
//!   forbids on the other path.
//!
//! Landing here stays exceptional either way: curation is broad, declines are
//! explicit, and a complaint is retired once the same `(raw_name, date)` stores
//! cleanly.

use sea_orm::{ActiveEnum, entity::prelude::*};
use serde::{Deserialize, Serialize};

use crate::entities::daily_metric::MetricKind;

/// The key under which every intake records *why* a point was refused.
pub const IMPORT_META_KEY: &str = "_import";

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "quarantined_metric")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// The metric name as received — HAE's `snake_case`, or Apple's `HK…`
    /// identifier. Usually one the vocabulary does not know, but a curated name
    /// lands here too when its point was refused; `_import.reason` says which.
    pub raw_name: String,
    /// The local calendar day parsed from the point (never UTC-shifted).
    pub date: Date,
    /// The entire HAE data point, verbatim (`serde_json::Value`).
    #[sea_orm(column_type = "Json")]
    pub raw_point: Json,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// Why a point could not be stored, recorded in `raw_point._import.reason`.
///
/// A closed vocabulary, so an enum rather than bare string literals: the
/// spellings are written by both intakes and read back by the backfill's
/// quarantine sweep, and a typo on either side would silently disable it — a
/// failure with no symptom other than a stale row nobody looks at.
///
/// It lives on the entity rather than in either intake because it describes
/// this row's own content, and both intakes write it. The stored spellings are
/// **stable**: rows carrying them already exist, and `parse` reads them back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
pub enum QuarantineReason {
    /// The metric name (HAE) or `type` attribute (`export.xml`) is not in the
    /// curated vocabulary.
    UnknownType,
    /// A `HKCategoryValueSleepAnalysis*` spelling we do not know.
    UnknownSleepStage,
    /// Curated name, but the point carried no unit at all.
    MissingUnit,
    /// Curated name, but no conversion covers its unit.
    UnconvertibleUnit,
    /// Curated name and a convertible unit, but the number is not finite.
    NonFiniteValue,
    /// Curated name and a convertible unit, but the number is outside what the
    /// kind can physically be (`services::plausibility`).
    ImplausibleValue,
}

impl QuarantineReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownType => "unknown-type",
            Self::UnknownSleepStage => "unknown-sleep-stage",
            Self::MissingUnit => "missing-unit",
            Self::UnconvertibleUnit => "unconvertible-unit",
            Self::NonFiniteValue => "non-finite-value",
            Self::ImplausibleValue => "implausible-value",
        }
    }

    /// Parse a reason back out of a stored row. `None` for anything this build
    /// does not recognize — rows written by some future build must not be
    /// mistaken for one of ours and swept.
    ///
    /// Enumerated rather than hand-listed (ADR-0003): a variant added to the
    /// enum but forgotten here would be written by one intake and then be
    /// unparsable to the sweep, with nothing failing to say so.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        use sea_orm::strum::IntoEnumIterator as _;
        Self::iter().find(|reason| reason.as_str() == raw)
    }

    /// Whether this reason describes the *name*, and so stops applying the
    /// moment that name joins the vocabulary.
    ///
    /// Deliberately not every reason: a curated metric can also be quarantined
    /// because a point carried an unconvertible unit or an impossible number,
    /// and those rows describe a live data problem that promoting the name does
    /// nothing about. Sweeping them would erase a standing complaint just
    /// because the metric happens to be curated.
    #[must_use]
    pub fn is_name_based(self) -> bool {
        matches!(self, Self::UnknownType | Self::UnknownSleepStage)
    }
}

/// What a refusal records about itself, stamped into `raw_point._import`.
///
/// The verbatim point alone is not always enough to act on. HAE carries
/// `units` on the *metric*, not the point, so a row quarantined for an
/// unconvertible unit would otherwise preserve everything except the one fact
/// needed to fix it — and unlike `export.xml`, the POST body is gone.
#[derive(Debug, Clone, Copy)]
pub struct QuarantineMeta<'a> {
    pub reason: QuarantineReason,
    /// The unit as the producer declared it, wherever the producer put it.
    pub units: Option<&'a str>,
    /// The curated kinds this point would have landed on. Empty when the name
    /// itself was never recognized; more than one when a single point explodes
    /// (`sleep_analysis`) and several stages were refused.
    pub kinds: &'a [MetricKind],
}

impl QuarantineMeta<'_> {
    /// Merge this into `point`'s `_import` object, creating it if absent and
    /// preserving any keys already there (the backfill's `records_seen`).
    pub fn stamp(&self, point: &mut Json) {
        let Some(obj) = point.as_object_mut() else {
            return;
        };
        let meta = obj
            .entry(IMPORT_META_KEY)
            .or_insert_with(|| Json::Object(serde_json::Map::new()));
        let Some(meta) = meta.as_object_mut() else {
            return;
        };
        meta.insert(
            "reason".to_owned(),
            Json::String(self.reason.as_str().to_owned()),
        );
        if let Some(units) = self.units {
            meta.insert("units".to_owned(), Json::String(units.to_owned()));
        }
        if !self.kinds.is_empty() {
            meta.insert(
                "kinds".to_owned(),
                Json::Array(
                    self.kinds
                        .iter()
                        .map(|k| Json::String(kind_slug(*k)))
                        .collect(),
                ),
            );
        }
    }
}

/// A kind's stored spelling, taken from the **same** `DeriveActiveEnum` value
/// the `daily_metric.kind` column stores.
///
/// Deliberately not `serde_json::to_value`, which would read the `#[serde(rename
/// = …)]` attribute — a second, parallel spelling list. The two agree today only
/// by convention, so reading the serde one would let a future edit to one
/// attribute and not the other put `gait_asymmetry` in a quarantine row against
/// a `gait-asymmetry` column, with nothing failing. `to_value` is also
/// infallible, so there is no fallback spelling to get wrong either.
fn kind_slug(kind: MetricKind) -> String {
    kind.to_value()
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

    use crate::{
        entities::quarantined_metric::{self},
        test_support::{date, datetime, test_db},
    };

    /// Every reason must survive the write/read cycle the backfill's sweep
    /// depends on. Enumerated, so a variant added without a spelling — or with
    /// one `parse` cannot recognize — fails here rather than silently
    /// disabling the sweep for that reason.
    #[test]
    fn every_reason_round_trips_through_its_stored_spelling() {
        use sea_orm::strum::IntoEnumIterator as _;

        use crate::entities::quarantined_metric::QuarantineReason;

        let mut seen = std::collections::BTreeSet::new();
        for reason in QuarantineReason::iter() {
            let spelling = reason.as_str();
            assert!(
                seen.insert(spelling),
                "{spelling} is used by two reasons — the sweep could not tell them apart"
            );
            assert_eq!(
                QuarantineReason::parse(spelling),
                Some(reason),
                "{reason:?} does not survive its own spelling"
            );
        }
        assert_eq!(seen.len(), 6, "a new reason needs a decision here");
        // Only the two NAME-based reasons may be swept when a name is promoted;
        // a bad unit or an impossible value is a standing complaint that
        // promoting the name does nothing about.
        assert!(QuarantineReason::UnknownType.is_name_based());
        assert!(QuarantineReason::UnknownSleepStage.is_name_based());
        for reason in [
            QuarantineReason::MissingUnit,
            QuarantineReason::UnconvertibleUnit,
            QuarantineReason::NonFiniteValue,
            QuarantineReason::ImplausibleValue,
        ] {
            assert!(
                !reason.is_name_based(),
                "{reason:?} must survive a promotion"
            );
        }
        assert_eq!(
            QuarantineReason::parse("something-a-future-build-wrote"),
            None
        );
    }

    #[tokio::test]
    async fn quarantined_metric_round_trips_json_and_unique_name_date() {
        let db = test_db().await;
        let now = datetime("2026-07-30 08:00:00");
        let point = serde_json::json!({
            "date": "2026-07-28 00:00:00 -0700",
            "qty": 42.0,
            "source": "Future Apple Metric",
        });
        quarantined_metric::ActiveModel {
            raw_name: Set("some_future_metric".to_owned()),
            date: Set(date("2026-07-28")),
            raw_point: Set(point.clone()),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert");

        // Explicit end-to-end proof the Json column round-trips content, not
        // just row count (no other entity exercises a Json column).
        let found = quarantined_metric::Entity::find()
            .all(&db)
            .await
            .expect("q");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].raw_name, "some_future_metric");
        assert_eq!(found[0].raw_point, point);

        // UNIQUE(raw_name, date): same name+date is rejected.
        let dup = quarantined_metric::ActiveModel {
            raw_name: Set("some_future_metric".to_owned()),
            date: Set(date("2026-07-28")),
            raw_point: Set(serde_json::json!({ "other": true })),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await;
        assert!(dup.is_err(), "duplicate (raw_name,date) must be rejected");
    }
}
