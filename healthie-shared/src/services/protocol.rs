use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::{now, today},
    entities::protocol,
    error::{DomainError, DomainResult},
    inputs::protocol::{NewProtocol, ProtocolOutcome},
    services::{concern, goal},
};

/// Loads a protocol by id.
///
/// # Errors
/// `DomainError::NotFound` if no protocol has id `id`; `DomainError::Db` on
/// database failure.
pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<protocol::Model> {
    protocol::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Protocol {id} not found")))
}

/// Starts a protocol, optionally linked to a concern and/or goal.
///
/// # Errors
/// `DomainError::Invalid` if `name` is empty; `DomainError::NotFound` if
/// `concern_id` or `goal_id` refers to no such record; `DomainError::Db` on
/// database failure.
pub async fn start(db: &impl ConnectionTrait, input: NewProtocol) -> DomainResult<protocol::Model> {
    if input.name.trim().is_empty() {
        return Err(DomainError::invalid("name", "must not be empty"));
    }
    if let Some(cid) = input.concern_id {
        concern::require(db, cid).await?;
    }
    if let Some(gid) = input.goal_id {
        goal::require(db, gid).await?;
    }
    Ok(protocol::ActiveModel {
        concern_id: Set(input.concern_id),
        goal_id: Set(input.goal_id),
        name: Set(input.name),
        kind: Set(input.kind),
        purpose: Set(input.purpose),
        schedule: Set(input.schedule),
        started_on: Set(input.started_on.unwrap_or_else(today)),
        review_by: Set(input.review_by),
        created_at: Set(now()),
        updated_at: Set(now()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

/// Records a protocol's final verdict, ending it.
///
/// # Errors
/// `DomainError::Invalid` if `rationale` is empty; `DomainError::NotFound` if no
/// protocol has id `id`; `DomainError::BadRequest` if the protocol already has a
/// verdict; `DomainError::Db` on database failure.
pub async fn record_outcome(
    db: &impl ConnectionTrait,
    id: i32,
    outcome: ProtocolOutcome,
) -> DomainResult<protocol::Model> {
    if outcome.rationale.trim().is_empty() {
        return Err(DomainError::invalid(
            "rationale",
            "required — the WHY is the permanent record",
        ));
    }
    let existing = require(db, id).await?;
    if existing.verdict.is_some() {
        return Err(DomainError::BadRequest(format!(
            "Protocol {id} already has a verdict; start a new protocol instead of rewriting \
             history"
        )));
    }
    let mut active: protocol::ActiveModel = existing.into();
    active.verdict = Set(Some(outcome.verdict));
    active.verdict_rationale = Set(Some(outcome.rationale));
    active.ended_on = Set(Some(outcome.ended_on.unwrap_or_else(today)));
    active.updated_at = Set(now());
    Ok(active.update(db).await?)
}

/// Lists protocols that have not yet ended.
///
/// # Errors
/// `DomainError::Db` on database failure.
pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<protocol::Model>> {
    Ok(protocol::Entity::find()
        .filter(protocol::Column::EndedOn.is_null())
        .order_by_asc(protocol::Column::Id)
        .all(db)
        .await?)
}

/// Lists every protocol, most recently started first.
///
/// # Errors
/// `DomainError::Db` on database failure.
pub async fn history(db: &impl ConnectionTrait) -> DomainResult<Vec<protocol::Model>> {
    Ok(protocol::Entity::find()
        .order_by_desc(protocol::Column::StartedOn)
        .order_by_desc(protocol::Column::Id)
        .all(db)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::protocol::{ProtocolKind, ProtocolVerdict},
        test_support::{date, test_db},
    };

    async fn keto(db: &sea_orm::DatabaseConnection) -> protocol::Model {
        start(
            db,
            NewProtocol {
                concern_id: None,
                goal_id: None,
                name: "Keto diet".into(),
                kind: ProtocolKind::Diet,
                purpose: Some("lose weight".into()),
                schedule: None,
                started_on: Some(date("2026-05-01")),
                review_by: None,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn outcome_requires_rationale_and_ends_protocol() {
        let db = test_db().await;
        let p = keto(&db).await;
        assert!(matches!(
            record_outcome(
                &db,
                p.id,
                ProtocolOutcome {
                    verdict: ProtocolVerdict::Mixed,
                    rationale: "  ".into(),
                    ended_on: None,
                }
            )
            .await,
            Err(DomainError::Invalid { .. })
        ));
        let done = record_outcome(
            &db,
            p.id,
            ProtocolOutcome {
                verdict: ProtocolVerdict::Mixed,
                rationale: "weight down but LDL up".into(),
                ended_on: None,
            },
        )
        .await
        .unwrap();
        assert!(done.ended_on.is_some());
        assert_eq!(done.verdict, Some(ProtocolVerdict::Mixed));
        assert!(list_active(&db).await.unwrap().is_empty());
        assert_eq!(history(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn outcome_rejects_double_verdict() {
        let db = test_db().await;
        let p = keto(&db).await;
        let outcome = ProtocolOutcome {
            verdict: ProtocolVerdict::Worked,
            rationale: "fine".into(),
            ended_on: None,
        };
        record_outcome(&db, p.id, outcome.clone()).await.unwrap();
        assert!(matches!(
            record_outcome(&db, p.id, outcome).await,
            Err(DomainError::BadRequest(_))
        ));
    }
}
