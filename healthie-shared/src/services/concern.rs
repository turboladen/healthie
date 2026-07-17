use std::fmt::Write as _;

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};
use serde::Serialize;

use crate::{
    clock::{now_str, today_str},
    entities::{concern, concern_tag},
    error::{DomainError, DomainResult},
    inputs::concern::NewConcern,
};

pub const VALID_STATUSES: [&str; 3] = ["active", "monitoring", "resolved"];
pub const VALID_TAGS: [&str; 10] = [
    "musculoskeletal",
    "neurological",
    "mental-health",
    "cardiovascular",
    "metabolic",
    "nutrition",
    "preventive",
    "immune",
    "sleep",
    "general",
];

#[derive(Debug, Serialize)]
pub struct ConcernWithTags {
    pub concern: concern::Model,
    pub tags: Vec<String>,
}

fn validate_tags(tags: &[String]) -> DomainResult<()> {
    for tag in tags {
        if !VALID_TAGS.contains(&tag.as_str()) {
            return Err(DomainError::BadRequest(format!(
                "Invalid tag '{tag}'. Must be one of: {}",
                VALID_TAGS.join(", ")
            )));
        }
    }
    Ok(())
}

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<concern::Model> {
    concern::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Concern {id} not found")))
}

pub async fn open(db: &impl ConnectionTrait, input: NewConcern) -> DomainResult<ConcernWithTags> {
    if input.name.trim().is_empty() {
        return Err(DomainError::invalid("name", "must not be empty"));
    }
    validate_tags(&input.tags)?;
    let model = concern::ActiveModel {
        name: Set(input.name),
        status: Set("active".into()),
        narrative: Set(input.narrative),
        opened_on: Set(input.opened_on.unwrap_or_else(today_str)),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?;
    let mut tags = Vec::new();
    for tag in input.tags {
        concern_tag::ActiveModel {
            concern_id: Set(model.id),
            tag: Set(tag.clone()),
            created_at: Set(now_str()),
            updated_at: Set(now_str()),
            ..Default::default()
        }
        .insert(db)
        .await?;
        tags.push(tag);
    }
    Ok(ConcernWithTags {
        concern: model,
        tags,
    })
}

pub async fn update_status(
    db: &impl ConnectionTrait,
    id: i32,
    status: &str,
    note: Option<String>,
) -> DomainResult<concern::Model> {
    if !VALID_STATUSES.contains(&status) {
        return Err(DomainError::BadRequest(format!(
            "Invalid status '{status}'. Must be one of: {}",
            VALID_STATUSES.join(", ")
        )));
    }
    let existing = require(db, id).await?;
    let mut narrative = existing.narrative.clone().unwrap_or_default();
    if let Some(note) = note {
        if !narrative.is_empty() {
            narrative.push('\n');
        }
        let _ = write!(narrative, "[{}] {note}", today_str());
    }
    let mut active: concern::ActiveModel = existing.into();
    active.status = Set(status.to_string());
    active.narrative = Set(if narrative.is_empty() {
        None
    } else {
        Some(narrative)
    });
    active.resolved_on = Set(if status == "resolved" {
        Some(today_str())
    } else {
        None
    });
    active.updated_at = Set(now_str());
    Ok(active.update(db).await?)
}

pub async fn tags_for(db: &impl ConnectionTrait, concern_id: i32) -> DomainResult<Vec<String>> {
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
        .filter(concern::Column::Status.ne("resolved"))
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
                tags: vec!["musculoskeletal".into(), "neurological".into()],
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
        assert_eq!(c.concern.status, "active");
        assert_eq!(c.tags, vec!["musculoskeletal", "neurological"]);
    }

    #[tokio::test]
    async fn open_rejects_unknown_tag() {
        let db = test_db().await;
        let res = open(
            &db,
            NewConcern {
                name: "X".into(),
                narrative: None,
                tags: vec!["chiropractics".into()],
                opened_on: None,
            },
        )
        .await;
        assert!(matches!(res, Err(DomainError::BadRequest(_))));
    }

    #[tokio::test]
    async fn update_status_resolves_and_appends_note() {
        let db = test_db().await;
        let c = seed(&db).await;
        let updated = update_status(&db, c.concern.id, "resolved", Some("PT finished".into()))
            .await
            .unwrap();
        assert_eq!(updated.status, "resolved");
        assert!(updated.resolved_on.is_some());
        assert!(updated.narrative.unwrap().contains("PT finished"));
        assert!(list_active(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_status_rejects_unknown_status_and_missing_id() {
        let db = test_db().await;
        let c = seed(&db).await;
        assert!(matches!(
            update_status(&db, c.concern.id, "cured", None).await,
            Err(DomainError::BadRequest(_))
        ));
        assert!(matches!(
            update_status(&db, 999, "active", None).await,
            Err(DomainError::NotFound(_))
        ));
    }
}
