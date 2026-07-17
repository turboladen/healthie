use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::now,
    entities::observation,
    error::{DomainError, DomainResult},
    inputs::observation::NewObservation,
    services::concern,
};

pub const VALID_ORIGINS: [&str; 3] = ["self", "ai", "rules"];
pub const VALID_KINDS: [&str; 2] = ["note", "symptom"];

pub async fn log(
    db: &impl ConnectionTrait,
    input: NewObservation,
) -> DomainResult<observation::Model> {
    if !VALID_ORIGINS.contains(&input.origin.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid origin '{}'. Must be one of: {}",
            input.origin,
            VALID_ORIGINS.join(", ")
        )));
    }
    if !VALID_KINDS.contains(&input.kind.as_str()) {
        return Err(DomainError::BadRequest(format!(
            "Invalid kind '{}'. Must be one of: {}",
            input.kind,
            VALID_KINDS.join(", ")
        )));
    }
    if input.body.trim().is_empty() {
        return Err(DomainError::invalid("body", "must not be empty"));
    }
    if let Some(s) = input.severity
        && !(1..=10).contains(&s)
    {
        return Err(DomainError::invalid("severity", "must be 1-10"));
    }
    if let Some(cid) = input.concern_id {
        concern::require(db, cid).await?;
    }
    let reviewed = i32::from(input.origin == "self");
    Ok(observation::ActiveModel {
        occurred_at: Set(input.occurred_at.unwrap_or_else(now)),
        origin: Set(input.origin),
        kind: Set(input.kind),
        body: Set(input.body),
        severity: Set(input.severity),
        concern_id: Set(input.concern_id),
        reviewed: Set(reviewed),
        created_at: Set(now()),
        updated_at: Set(now()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn pending_review(db: &impl ConnectionTrait) -> DomainResult<Vec<observation::Model>> {
    Ok(observation::Entity::find()
        .filter(observation::Column::Reviewed.eq(0))
        .order_by_asc(observation::Column::OccurredAt)
        .all(db)
        .await?)
}

pub async fn mark_reviewed(db: &impl ConnectionTrait, id: i32) -> DomainResult<observation::Model> {
    let existing = observation::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Observation {id} not found")))?;
    let mut active: observation::ActiveModel = existing.into();
    active.reviewed = Set(1);
    active.updated_at = Set(now());
    Ok(active.update(db).await?)
}

pub async fn recent(
    db: &impl ConnectionTrait,
    since: DateTime<Utc>,
) -> DomainResult<Vec<observation::Model>> {
    Ok(observation::Entity::find()
        .filter(observation::Column::OccurredAt.gte(since))
        .order_by_desc(observation::Column::OccurredAt)
        .all(db)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{datetime, test_db};

    fn spasm(origin: &str) -> NewObservation {
        NewObservation {
            origin: origin.into(),
            kind: "symptom".into(),
            body: "Back spasm getting out of the car".into(),
            severity: Some(6),
            concern_id: None,
            occurred_at: None,
        }
    }

    #[tokio::test]
    async fn self_observations_need_no_review_but_ai_do() {
        let db = test_db().await;
        log(&db, spasm("self")).await.unwrap();
        let ai = log(
            &db,
            NewObservation {
                origin: "ai".into(),
                kind: "note".into(),
                body: "Resting HR elevated since Tuesday".into(),
                severity: None,
                concern_id: None,
                occurred_at: None,
            },
        )
        .await
        .unwrap();
        let pending = pending_review(&db).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, ai.id);
        mark_reviewed(&db, ai.id).await.unwrap();
        assert!(pending_review(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn log_validates_origin_kind_severity() {
        let db = test_db().await;
        assert!(matches!(
            log(&db, spasm("claude")).await,
            Err(DomainError::BadRequest(_))
        ));
        let mut bad_kind = spasm("self");
        bad_kind.kind = "feeling".into();
        assert!(matches!(
            log(&db, bad_kind).await,
            Err(DomainError::BadRequest(_))
        ));
        let mut bad_sev = spasm("self");
        bad_sev.severity = Some(11);
        assert!(matches!(
            log(&db, bad_sev).await,
            Err(DomainError::Invalid { .. })
        ));
    }

    #[tokio::test]
    async fn recent_filters_by_date() {
        let db = test_db().await;
        let mut old = spasm("self");
        old.occurred_at = Some(datetime("2026-01-01 08:00:00"));
        log(&db, old).await.unwrap();
        log(&db, spasm("self")).await.unwrap(); // now
        assert_eq!(
            recent(&db, datetime("2026-06-01 00:00:00"))
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
