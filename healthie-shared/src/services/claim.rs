//! Claims registry services (ADR-0004). All writes validate here; the MCP
//! layer only maps shapes. `record_batch` is transactional: a bad claim in
//! the batch persists nothing.

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait, strum::IntoEnumIterator as _,
};
use serde::Serialize;

use crate::{
    clock,
    entities::claim::{self, ClaimCategory, ClaimConfidence},
    error::{DomainError, DomainResult},
    inputs::claim::{ClaimFilter, NewClaim, UpdateClaim},
    services::concern,
};

/// Per-category intake coverage, derived entirely from the registry —
/// the sessionless intake state (ADR-0004).
#[derive(Debug, Serialize)]
pub struct CategoryCoverage {
    pub category: ClaimCategory,
    pub claims: usize,
    pub unknowns: usize,
    pub last_touched: Option<DateTime<Utc>>,
}

/// A stored `subject` must be a real relative name: claims about Steve carry
/// NULL, so the literal "self" (any case) or a blank string would create rows
/// invisible to the self scope. The MCP boundary canonicalizes these away for
/// its callers; this domain guard defends the invariant against every OTHER
/// write path (M2 backend, tests, future surfaces) — ADR-0004 §2.
fn validate_subject(subject: Option<&str>) -> DomainResult<()> {
    if let Some(subject) = subject {
        let trimmed = subject.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("self") {
            return Err(DomainError::invalid(
                "subject",
                "must be a relative's name, or omitted for claims about Steve",
            ));
        }
    }
    Ok(())
}

/// Record a batch of claims in one transaction.
///
/// # Errors
/// `Invalid` on an empty batch, any blank `statement`, or a `subject` that is
/// blank or the reserved literal "self"; `NotFound` if a `concern_id` doesn't
/// exist; `Db` on database errors. Nothing persists unless every claim is
/// valid.
pub async fn record_batch<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    inputs: Vec<NewClaim>,
) -> DomainResult<Vec<claim::Model>> {
    if inputs.is_empty() {
        return Err(DomainError::invalid(
            "claims",
            "at least one claim is required",
        ));
    }
    for input in &inputs {
        if input.statement.trim().is_empty() {
            return Err(DomainError::invalid("statement", "must not be empty"));
        }
        validate_subject(input.subject.as_deref())?;
    }
    let txn = db.begin().await?;
    let now = clock::now();
    let mut saved = Vec::with_capacity(inputs.len());
    for input in inputs {
        if let Some(concern_id) = input.concern_id {
            concern::require(&txn, concern_id).await?;
        }
        let model = claim::ActiveModel {
            category: Set(input.category),
            statement: Set(input.statement),
            confidence: Set(input.confidence),
            subject: Set(input.subject),
            topic: Set(input.topic),
            occurred_on: Set(input.occurred_on),
            source_quote: Set(input.source_quote),
            concern_id: Set(input.concern_id),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&txn)
        .await?;
        saved.push(model);
    }
    txn.commit().await?;
    Ok(saved)
}

/// Revise a claim. `source_quote` is deliberately not revisable — it is the
/// evidence the claim was distilled from.
///
/// # Errors
/// `Invalid` for a blank `statement` or a `subject` set to blank or the
/// reserved literal "self"; `NotFound` for a missing claim id or a missing
/// `concern_id` target; `Db` on database errors.
pub async fn update(
    db: &impl ConnectionTrait,
    id: i32,
    input: UpdateClaim,
) -> DomainResult<claim::Model> {
    let existing = claim::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("claim {id} not found")))?;
    if let Some(statement) = &input.statement
        && statement.trim().is_empty()
    {
        return Err(DomainError::invalid("statement", "must not be empty"));
    }
    if let Some(subject) = &input.subject {
        validate_subject(subject.as_deref())?;
    }
    if let Some(Some(concern_id)) = input.concern_id {
        concern::require(db, concern_id).await?;
    }
    let mut active: claim::ActiveModel = existing.into();
    if let Some(v) = input.category {
        active.category = Set(v);
    }
    if let Some(v) = input.statement {
        active.statement = Set(v);
    }
    if let Some(v) = input.confidence {
        active.confidence = Set(v);
    }
    if let Some(v) = input.subject {
        active.subject = Set(v);
    }
    if let Some(v) = input.topic {
        active.topic = Set(v);
    }
    if let Some(v) = input.occurred_on {
        active.occurred_on = Set(v);
    }
    if let Some(v) = input.concern_id {
        active.concern_id = Set(v);
    }
    active.updated_at = Set(clock::now());
    Ok(active.update(db).await?)
}

/// List claims, newest first, with optional filters.
///
/// # Errors
/// `Db` on database errors.
pub async fn list(
    db: &impl ConnectionTrait,
    filter: ClaimFilter,
) -> DomainResult<Vec<claim::Model>> {
    let mut query = claim::Entity::find().order_by_desc(claim::Column::Id);
    if let Some(category) = filter.category {
        query = query.filter(claim::Column::Category.eq(category));
    }
    if let Some(confidence) = filter.confidence {
        query = query.filter(claim::Column::Confidence.eq(confidence));
    }
    match filter.subject {
        Some(None) => query = query.filter(claim::Column::Subject.is_null()),
        Some(Some(subject)) => query = query.filter(claim::Column::Subject.eq(subject)),
        None => {}
    }
    Ok(query.all(db).await?)
}

/// The briefing's "unknowns to resolve" list — never a nag, always visible.
///
/// # Errors
/// `Db` on database errors.
pub async fn unresolved(db: &impl ConnectionTrait) -> DomainResult<Vec<claim::Model>> {
    list(
        db,
        ClaimFilter {
            confidence: Some(ClaimConfidence::Unknown),
            ..ClaimFilter::default()
        },
    )
    .await
}

/// Intake coverage per category — includes zero-claim categories so the
/// intake prompt can see what was never visited. Aggregated in memory (the
/// registry is small by construction).
///
/// # Errors
/// `Db` on database errors.
pub async fn coverage(db: &impl ConnectionTrait) -> DomainResult<Vec<CategoryCoverage>> {
    let all = claim::Entity::find().all(db).await?;
    Ok(ClaimCategory::iter()
        .map(|category| {
            let in_category: Vec<_> = all.iter().filter(|c| c.category == category).collect();
            let unknowns = in_category
                .iter()
                .filter(|c| c.confidence == ClaimConfidence::Unknown)
                .count();
            CategoryCoverage {
                category,
                claims: in_category.len(),
                unknowns,
                last_touched: in_category.iter().map(|c| c.updated_at).max(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::claim::{ClaimCategory, ClaimConfidence},
        inputs::concern::NewConcern,
        services::concern,
        test_support::{date, test_db},
    };

    fn father_afib() -> NewClaim {
        NewClaim {
            category: ClaimCategory::FamilyHistory,
            statement: "Father: afib onset in his 60s".to_owned(),
            confidence: ClaimConfidence::Recalled,
            subject: Some("father".to_owned()),
            topic: Some("afib".to_owned()),
            occurred_on: None,
            source_quote: Some("I think my dad's afib started in his 60s?".to_owned()),
            concern_id: None,
        }
    }

    fn colonoscopy_unknown() -> NewClaim {
        NewClaim {
            category: ClaimCategory::Screening,
            statement: "Colonoscopy: never had one? unsure".to_owned(),
            confidence: ClaimConfidence::Unknown,
            subject: None,
            topic: Some("colonoscopy".to_owned()),
            occurred_on: None,
            source_quote: None,
            concern_id: None,
        }
    }

    #[tokio::test]
    async fn record_batch_persists_all_in_one_txn() {
        let db = test_db().await;
        let saved = record_batch(&db, vec![father_afib(), colonoscopy_unknown()])
            .await
            .expect("record");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].category, ClaimCategory::FamilyHistory);
    }

    #[tokio::test]
    async fn record_batch_rejects_empty_batch_and_blank_statement() {
        let db = test_db().await;
        assert!(
            record_batch(&db, vec![]).await.is_err(),
            "empty batch is Invalid"
        );
        let mut blank = father_afib();
        blank.statement = "   ".to_owned();
        let result = record_batch(&db, vec![colonoscopy_unknown(), blank]).await;
        assert!(result.is_err(), "blank statement is Invalid");
        // Statement validation is pre-txn (before begin), so nothing is even
        // attempted — this pins pre-txn rejection, not rollback. In-txn
        // rollback is covered by record_batch_rolls_back_inserted_claims.
        assert!(
            list(&db, ClaimFilter::default())
                .await
                .expect("list")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn record_batch_rolls_back_inserted_claims() {
        let db = test_db().await;
        // First claim is valid and gets inserted inside the txn; the second
        // fails concern::require AFTER that insert. The commit never runs, so
        // the already-inserted first claim must be rolled back.
        let mut second = father_afib();
        second.concern_id = Some(9999);
        let result = record_batch(&db, vec![colonoscopy_unknown(), second]).await;
        assert!(result.is_err(), "missing concern must fail the batch");
        assert!(
            list(&db, ClaimFilter::default())
                .await
                .expect("list")
                .is_empty(),
            "an in-txn failure must roll back the already-inserted claim"
        );
    }

    #[tokio::test]
    async fn record_batch_validates_concern_exists() {
        let db = test_db().await;
        let mut bad = father_afib();
        bad.concern_id = Some(9999);
        assert!(record_batch(&db, vec![bad]).await.is_err());

        let opened = concern::open(
            &db,
            NewConcern {
                name: "Cardio".to_owned(),
                narrative: None,
                tags: vec![],
                opened_on: Some(date("2026-07-18")),
            },
        )
        .await
        .expect("concern");
        let mut good = father_afib();
        good.concern_id = Some(opened.concern.id);
        assert!(record_batch(&db, vec![good]).await.is_ok());
    }

    #[tokio::test]
    async fn update_revises_fields_but_not_source_quote() {
        let db = test_db().await;
        let saved = record_batch(&db, vec![father_afib()])
            .await
            .expect("record");
        let updated = update(
            &db,
            saved[0].id,
            UpdateClaim {
                confidence: Some(ClaimConfidence::Verified),
                statement: Some("Father: afib confirmed, diagnosed at 63".to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("update");
        assert_eq!(updated.confidence, ClaimConfidence::Verified);
        // provenance untouched (and untouchable — UpdateClaim has no field for it)
        assert_eq!(updated.source_quote, saved[0].source_quote);
        assert!(updated.updated_at > saved[0].updated_at);
    }

    #[tokio::test]
    async fn update_all_none_bumps_only_updated_at() {
        let db = test_db().await;
        let saved = record_batch(&db, vec![father_afib()])
            .await
            .expect("record");
        let before = &saved[0];
        let updated = update(&db, before.id, UpdateClaim::default())
            .await
            .expect("no-op update still succeeds");
        assert_eq!(updated.category, before.category);
        assert_eq!(updated.statement, before.statement);
        assert_eq!(updated.confidence, before.confidence);
        assert_eq!(updated.subject, before.subject);
        assert_eq!(updated.topic, before.topic);
        assert_eq!(updated.source_quote, before.source_quote);
        assert!(updated.updated_at > before.updated_at);
    }

    /// The domain guard behind the MCP-boundary canonicalization (ADR-0004
    /// §2): "self" or blank must never persist as a stored subject, no
    /// matter which write path a future surface takes.
    #[tokio::test]
    async fn record_batch_rejects_reserved_or_blank_subject() {
        let db = test_db().await;
        let mut reserved = father_afib();
        reserved.subject = Some(" SELF ".to_owned());
        assert!(
            record_batch(&db, vec![reserved]).await.is_err(),
            "the reserved literal must be rejected regardless of case/padding"
        );
        let mut blank = father_afib();
        blank.subject = Some("   ".to_owned());
        assert!(
            record_batch(&db, vec![blank]).await.is_err(),
            "a blank subject must be rejected"
        );
    }

    #[tokio::test]
    async fn update_rejects_reserved_or_blank_subject_but_allows_clear() {
        let db = test_db().await;
        let saved = record_batch(&db, vec![father_afib()])
            .await
            .expect("record");
        let reserved = update(
            &db,
            saved[0].id,
            UpdateClaim {
                subject: Some(Some("self".to_owned())),
                ..Default::default()
            },
        )
        .await;
        assert!(reserved.is_err(), "literal 'self' must not persist");
        let blank = update(
            &db,
            saved[0].id,
            UpdateClaim {
                subject: Some(Some(" ".to_owned())),
                ..Default::default()
            },
        )
        .await;
        assert!(blank.is_err(), "blank subject must not persist");
        // Some(None) stays the legitimate domain-level clear.
        let cleared = update(
            &db,
            saved[0].id,
            UpdateClaim {
                subject: Some(None),
                ..Default::default()
            },
        )
        .await
        .expect("clear via Some(None)");
        assert_eq!(cleared.subject, None);
    }

    #[tokio::test]
    async fn update_not_found_and_blank_statement_rejected() {
        let db = test_db().await;
        assert!(update(&db, 9999, UpdateClaim::default()).await.is_err());
        let saved = record_batch(&db, vec![father_afib()])
            .await
            .expect("record");
        let result = update(
            &db,
            saved[0].id,
            UpdateClaim {
                statement: Some("  ".to_owned()),
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_filters_by_category_confidence_and_subject_scope() {
        let db = test_db().await;
        record_batch(&db, vec![father_afib(), colonoscopy_unknown()])
            .await
            .expect("record");
        let fam = list(
            &db,
            ClaimFilter {
                category: Some(ClaimCategory::FamilyHistory),
                ..Default::default()
            },
        )
        .await
        .expect("list");
        assert_eq!(fam.len(), 1);
        let unknowns = list(
            &db,
            ClaimFilter {
                confidence: Some(ClaimConfidence::Unknown),
                ..Default::default()
            },
        )
        .await
        .expect("list");
        assert_eq!(unknowns.len(), 1);
        let self_only = list(
            &db,
            ClaimFilter {
                subject: Some(None),
                ..Default::default()
            },
        )
        .await
        .expect("list");
        assert_eq!(self_only.len(), 1, "subject IS NULL means about Steve");
        let father = list(
            &db,
            ClaimFilter {
                subject: Some(Some("father".to_owned())),
                ..Default::default()
            },
        )
        .await
        .expect("list");
        assert_eq!(father.len(), 1);
    }

    #[tokio::test]
    async fn unresolved_returns_only_unknowns() {
        let db = test_db().await;
        record_batch(&db, vec![father_afib(), colonoscopy_unknown()])
            .await
            .expect("record");
        let unresolved = unresolved(&db).await.expect("unresolved");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].confidence, ClaimConfidence::Unknown);
    }

    #[tokio::test]
    async fn coverage_includes_every_category_even_empty() {
        let db = test_db().await;
        record_batch(&db, vec![father_afib(), colonoscopy_unknown()])
            .await
            .expect("record");
        let cov = coverage(&db).await.expect("coverage");
        assert_eq!(cov.len(), 11, "one row per ClaimCategory variant");
        let fam = cov
            .iter()
            .find(|c| c.category == ClaimCategory::FamilyHistory)
            .expect("family row");
        assert_eq!((fam.claims, fam.unknowns), (1, 0));
        assert!(fam.last_touched.is_some());
        let screening = cov
            .iter()
            .find(|c| c.category == ClaimCategory::Screening)
            .expect("screening row");
        assert_eq!((screening.claims, screening.unknowns), (1, 1));
        let surgery = cov
            .iter()
            .find(|c| c.category == ClaimCategory::Surgery)
            .expect("surgery row");
        assert_eq!((surgery.claims, surgery.unknowns), (0, 0));
        assert!(surgery.last_touched.is_none());
    }

    #[tokio::test]
    async fn coverage_last_touched_reads_updated_at_not_created_at() {
        let db = test_db().await;
        let saved = record_batch(&db, vec![father_afib()])
            .await
            .expect("record");
        let before = coverage(&db)
            .await
            .expect("coverage")
            .into_iter()
            .find(|c| c.category == ClaimCategory::FamilyHistory)
            .expect("family row")
            .last_touched
            .expect("touched");
        // A revision advances updated_at; coverage must reflect it (not created_at).
        update(
            &db,
            saved[0].id,
            UpdateClaim {
                confidence: Some(ClaimConfidence::Verified),
                ..Default::default()
            },
        )
        .await
        .expect("update");
        let after = coverage(&db)
            .await
            .expect("coverage")
            .into_iter()
            .find(|c| c.category == ClaimCategory::FamilyHistory)
            .expect("family row")
            .last_touched
            .expect("touched");
        assert!(
            after > before,
            "last_touched must track updated_at (not created_at) across a revision"
        );
    }
}
