use std::fmt::Write as _;

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait,
};
use serde::Serialize;

use crate::{
    clock::{now, today},
    entities::{
        concern::{self, ConcernStatus},
        concern_tag::{self, ConcernTag},
    },
    error::{DomainError, DomainResult},
    inputs::concern::NewConcern,
};

#[derive(Debug, Serialize)]
pub struct ConcernWithTags {
    pub concern: concern::Model,
    pub tags: Vec<ConcernTag>,
}

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<concern::Model> {
    concern::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Concern {id} not found")))
}

pub async fn open<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    input: NewConcern,
) -> DomainResult<ConcernWithTags> {
    if input.name.trim().is_empty() {
        return Err(DomainError::invalid("name", "must not be empty"));
    }
    // Dedupe order-preserving, first-wins: the (concern_id, tag) unique index
    // rejects duplicates, so silently collapse them rather than fail the insert.
    let mut tags: Vec<ConcernTag> = Vec::new();
    for tag in input.tags {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }

    // concern + tags are one unit of work: a mid-loop failure must not leave a
    // persisted concern with only some of its tags.
    let txn = db.begin().await?;
    let model = concern::ActiveModel {
        name: Set(input.name),
        status: Set(ConcernStatus::Active),
        narrative: Set(input.narrative),
        opened_on: Set(input.opened_on.unwrap_or_else(today)),
        created_at: Set(now()),
        updated_at: Set(now()),
        ..Default::default()
    }
    .insert(&txn)
    .await?;
    for tag in &tags {
        concern_tag::ActiveModel {
            concern_id: Set(model.id),
            tag: Set(*tag),
            created_at: Set(now()),
            updated_at: Set(now()),
            ..Default::default()
        }
        .insert(&txn)
        .await?;
    }
    txn.commit().await?;
    Ok(ConcernWithTags {
        concern: model,
        tags,
    })
}

pub async fn update_status(
    db: &impl ConnectionTrait,
    id: i32,
    status: ConcernStatus,
    note: Option<String>,
) -> DomainResult<concern::Model> {
    let existing = require(db, id).await?;
    let mut narrative = existing.narrative.clone().unwrap_or_default();
    if let Some(note) = note {
        if !narrative.is_empty() {
            narrative.push('\n');
        }
        let _ = write!(narrative, "[{}] {note}", today());
    }
    let mut active: concern::ActiveModel = existing.into();
    active.status = Set(status);
    active.narrative = Set(if narrative.is_empty() {
        None
    } else {
        Some(narrative)
    });
    active.resolved_on = Set(if status == ConcernStatus::Resolved {
        Some(today())
    } else {
        None
    });
    active.updated_at = Set(now());
    Ok(active.update(db).await?)
}

pub async fn tags_for(db: &impl ConnectionTrait, concern_id: i32) -> DomainResult<Vec<ConcernTag>> {
    Ok(concern_tag::Entity::find()
        .filter(concern_tag::Column::ConcernId.eq(concern_id))
        .order_by_asc(concern_tag::Column::Id)
        .all(db)
        .await?
        .into_iter()
        .map(|t| t.tag)
        .collect())
}

pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<ConcernWithTags>> {
    let concerns = concern::Entity::find()
        .filter(concern::Column::Status.ne(ConcernStatus::Resolved))
        .order_by_asc(concern::Column::Id)
        .all(db)
        .await?;
    let mut out = Vec::with_capacity(concerns.len());
    for c in concerns {
        let tags = tags_for(db, c.id).await?;
        out.push(ConcernWithTags { concern: c, tags });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    async fn seed(db: &sea_orm::DatabaseConnection) -> ConcernWithTags {
        open(
            db,
            NewConcern {
                name: "Bad back".into(),
                narrative: Some("L4/L5 disc".into()),
                tags: vec![ConcernTag::Musculoskeletal, ConcernTag::Neurological],
                opened_on: None,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn open_stores_concern_with_tags() {
        let db = test_db().await;
        let c = seed(&db).await;
        assert_eq!(c.concern.status, ConcernStatus::Active);
        assert_eq!(
            c.tags,
            vec![ConcernTag::Musculoskeletal, ConcernTag::Neurological]
        );
    }

    #[tokio::test]
    async fn open_dedupes_duplicate_tags() {
        let db = test_db().await;
        let c = open(
            &db,
            NewConcern {
                name: "Bad back".into(),
                narrative: None,
                tags: vec![
                    ConcernTag::Musculoskeletal,
                    ConcernTag::Musculoskeletal,
                    ConcernTag::Neurological,
                ],
                opened_on: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            c.tags,
            vec![ConcernTag::Musculoskeletal, ConcernTag::Neurological]
        );
        // stored once, not once per duplicate
        let reloaded = tags_for(&db, c.concern.id).await.unwrap();
        assert_eq!(
            reloaded,
            vec![ConcernTag::Musculoskeletal, ConcernTag::Neurological]
        );
    }

    #[tokio::test]
    async fn update_status_resolves_and_appends_note() {
        let db = test_db().await;
        let c = seed(&db).await;
        let updated = update_status(
            &db,
            c.concern.id,
            ConcernStatus::Resolved,
            Some("PT finished".into()),
        )
        .await
        .unwrap();
        assert_eq!(updated.status, ConcernStatus::Resolved);
        assert!(updated.resolved_on.is_some());
        assert!(updated.narrative.unwrap().contains("PT finished"));
        assert!(list_active(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_status_rejects_missing_id() {
        let db = test_db().await;
        assert!(matches!(
            update_status(&db, 999, ConcernStatus::Active, None).await,
            Err(DomainError::NotFound(_))
        ));
    }
}
