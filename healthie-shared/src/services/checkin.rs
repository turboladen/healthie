use chrono::NaiveTime;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::{now, today},
    entities::{checkin, checkin_response},
    error::{DomainError, DomainResult},
    services::concern,
};

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<checkin::Model> {
    checkin::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Checkin {id} not found")))
}

/// Opens today's checkin, or resumes an incomplete one started today.
pub async fn start(db: &impl ConnectionTrait) -> DomainResult<checkin::Model> {
    let day_start = today().and_time(NaiveTime::MIN).and_utc();
    let open = checkin::Entity::find()
        .filter(checkin::Column::CompletedAt.is_null())
        .filter(checkin::Column::StartedAt.gte(day_start))
        .one(db)
        .await?;
    if let Some(existing) = open {
        return Ok(existing);
    }
    Ok(checkin::ActiveModel {
        started_at: Set(now()),
        created_at: Set(now()),
        updated_at: Set(now()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn record_response(
    db: &impl ConnectionTrait,
    checkin_id: i32,
    question: &str,
    answer: &str,
    concern_id: Option<i32>,
) -> DomainResult<checkin_response::Model> {
    let ck = require(db, checkin_id).await?;
    if ck.completed_at.is_some() {
        return Err(DomainError::BadRequest(format!(
            "Checkin {checkin_id} is already completed; start a new checkin"
        )));
    }
    if answer.trim().is_empty() {
        return Err(DomainError::invalid("answer", "must not be empty"));
    }
    if let Some(cid) = concern_id {
        concern::require(db, cid).await?;
    }
    Ok(checkin_response::ActiveModel {
        checkin_id: Set(checkin_id),
        question: Set(question.to_string()),
        answer: Set(answer.to_string()),
        concern_id: Set(concern_id),
        created_at: Set(now()),
        updated_at: Set(now()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn complete(
    db: &impl ConnectionTrait,
    checkin_id: i32,
    summary: &str,
) -> DomainResult<checkin::Model> {
    let ck = require(db, checkin_id).await?;
    if ck.completed_at.is_some() {
        return Err(DomainError::BadRequest(format!(
            "Checkin {checkin_id} is already completed"
        )));
    }
    let mut active: checkin::ActiveModel = ck.into();
    active.completed_at = Set(Some(now()));
    active.summary = Set(Some(summary.to_string()));
    active.updated_at = Set(now());
    Ok(active.update(db).await?)
}

pub async fn latest_completed(
    db: &impl ConnectionTrait,
) -> DomainResult<Option<(checkin::Model, Vec<checkin_response::Model>)>> {
    let latest = checkin::Entity::find()
        .filter(checkin::Column::CompletedAt.is_not_null())
        .order_by_desc(checkin::Column::CompletedAt)
        .order_by_desc(checkin::Column::Id)
        .one(db)
        .await?;
    let Some(ck) = latest else {
        return Ok(None);
    };
    let responses = checkin_response::Entity::find()
        .filter(checkin_response::Column::CheckinId.eq(ck.id))
        .order_by_asc(checkin_response::Column::Id)
        .all(db)
        .await?;
    Ok(Some((ck, responses)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    #[tokio::test]
    async fn start_is_idempotent_within_a_day() {
        let db = test_db().await;
        let a = start(&db).await.unwrap();
        let b = start(&db).await.unwrap();
        assert_eq!(a.id, b.id); // resumed, not duplicated
        complete(&db, a.id, "ok week").await.unwrap();
        let c = start(&db).await.unwrap();
        assert_ne!(a.id, c.id); // completed one is closed; new checkin opens
    }

    #[tokio::test]
    async fn responses_append_and_lock_after_complete() {
        let db = test_db().await;
        let ck = start(&db).await.unwrap();
        record_response(
            &db,
            ck.id,
            "How was your week?",
            "Rough — back flared.",
            None,
        )
        .await
        .unwrap();
        record_response(&db, ck.id, "Sleep?", "Bad, kids sick.", None)
            .await
            .unwrap();
        complete(&db, ck.id, "Back flare, poor sleep.")
            .await
            .unwrap();
        assert!(matches!(
            record_response(&db, ck.id, "One more?", "no", None).await,
            Err(DomainError::BadRequest(_))
        ));
        let (latest, responses) = latest_completed(&db).await.unwrap().unwrap();
        assert_eq!(latest.id, ck.id);
        assert_eq!(responses.len(), 2);
        assert_eq!(latest.summary.as_deref(), Some("Back flare, poor sleep."));
    }
}
