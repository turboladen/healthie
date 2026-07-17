use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::{now_str, today_str},
    entities::protocol,
    error::{DomainError, DomainResult},
    inputs::protocol::{NewProtocol, ProtocolOutcome},
    services::{concern, goal},
};

pub const VALID_KINDS: [&str; 6] = [
    "diet",
    "exercise",
    "supplement",
    "therapy",
    "screening",
    "habit",
];
pub const VALID_VERDICTS: [&str; 4] = ["worked", "didnt-work", "mixed", "inconclusive"];

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<protocol::Model> {
    protocol::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Protocol {id} not found")))
}

pub async fn start(db: &impl ConnectionTrait, input: NewProtocol) -> DomainResult<protocol::Model> {
    if input.name.trim().is_empty() {
        return Err(DomainError::invalid("name", "must not be empty"));
    }
    if !VALID_KINDS.contains(&input.kind.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid kind '{}'. Must be one of: {}",
            input.kind,
            VALID_KINDS.join(", ")
        )));
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
        started_on: Set(input.started_on.unwrap_or_else(today_str)),
        review_by: Set(input.review_by),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn record_outcome(
    db: &impl ConnectionTrait,
    id: i32,
    outcome: ProtocolOutcome,
) -> DomainResult<protocol::Model> {
    if !VALID_VERDICTS.contains(&outcome.verdict.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid verdict '{}'. Must be one of: {}",
            outcome.verdict,
            VALID_VERDICTS.join(", ")
        )));
    }
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
    active.ended_on = Set(Some(outcome.ended_on.unwrap_or_else(today_str)));
    active.updated_at = Set(now_str());
    Ok(active.update(db).await?)
}

pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<protocol::Model>> {
    Ok(protocol::Entity::find()
        .filter(protocol::Column::EndedOn.is_null())
        .order_by_asc(protocol::Column::Id)
        .all(db)
        .await?)
}

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
    use crate::test_support::test_db;

    async fn keto(db: &sea_orm::DatabaseConnection) -> protocol::Model {
        start(
            db,
            NewProtocol {
                concern_id: None,
                goal_id: None,
                name: "Keto diet".into(),
                kind: "diet".into(),
                purpose: Some("lose weight".into()),
                schedule: None,
                started_on: Some("2026-05-01".into()),
                review_by: None,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn start_rejects_unknown_kind() {
        let db = test_db().await;
        let res = start(
            &db,
            NewProtocol {
                concern_id: None,
                goal_id: None,
                name: "X".into(),
                kind: "regimen".into(),
                purpose: None,
                schedule: None,
                started_on: None,
                review_by: None,
            },
        )
        .await;
        assert!(matches!(res, Err(DomainError::BadRequest(_))));
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
                    verdict: "mixed".into(),
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
                verdict: "mixed".into(),
                rationale: "weight down but LDL up".into(),
                ended_on: None,
            },
        )
        .await
        .unwrap();
        assert!(done.ended_on.is_some());
        assert_eq!(done.verdict.as_deref(), Some("mixed"));
        assert!(list_active(&db).await.unwrap().is_empty());
        assert_eq!(history(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn outcome_rejects_double_verdict() {
        let db = test_db().await;
        let p = keto(&db).await;
        let outcome = ProtocolOutcome {
            verdict: "worked".into(),
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
