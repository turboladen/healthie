use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::now,
    entities::goal,
    error::{DomainError, DomainResult},
    inputs::goal::NewGoal,
    services::concern,
};

pub const VALID_STATUSES: [&str; 4] = ["active", "achieved", "abandoned", "paused"];
pub const VALID_COMPARISONS: [&str; 3] = ["at-most", "at-least", "range"];

pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<goal::Model> {
    goal::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Goal {id} not found")))
}

pub async fn set(db: &impl ConnectionTrait, input: NewGoal) -> DomainResult<goal::Model> {
    if input.title.trim().is_empty() {
        return Err(DomainError::invalid("title", "must not be empty"));
    }
    if let Some(cid) = input.concern_id {
        concern::require(db, cid).await?;
    }
    if let Some(cmp) = &input.comparison {
        if !VALID_COMPARISONS.contains(&cmp.as_str()) {
            return Err(DomainError::BadRequest(format!(
                "Invalid comparison '{cmp}'. Must be one of: {}",
                VALID_COMPARISONS.join(", ")
            )));
        }
        if input.target_value.is_none() {
            return Err(DomainError::invalid(
                "target_value",
                "required when comparison is set",
            ));
        }
        if cmp == "range" && input.target_high.is_none() {
            return Err(DomainError::invalid(
                "target_high",
                "required when comparison is 'range'",
            ));
        }
        if cmp == "range"
            && let (Some(low), Some(high)) = (input.target_value, input.target_high)
            && low > high
        {
            return Err(DomainError::invalid(
                "target_high",
                "must be >= target_value",
            ));
        }
    }
    Ok(goal::ActiveModel {
        concern_id: Set(input.concern_id),
        title: Set(input.title),
        description: Set(input.description),
        metric_kind: Set(input.metric_kind),
        comparison: Set(input.comparison),
        target_value: Set(input.target_value),
        target_high: Set(input.target_high),
        target_date: Set(input.target_date),
        status: Set("active".into()),
        created_at: Set(now()),
        updated_at: Set(now()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn update_status(
    db: &impl ConnectionTrait,
    id: i32,
    status: &str,
) -> DomainResult<goal::Model> {
    if !VALID_STATUSES.contains(&status) {
        return Err(DomainError::BadRequest(format!(
            "Invalid status '{status}'. Must be one of: {}",
            VALID_STATUSES.join(", ")
        )));
    }
    let mut active: goal::ActiveModel = require(db, id).await?.into();
    active.status = Set(status.to_string());
    active.updated_at = Set(now());
    Ok(active.update(db).await?)
}

pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<goal::Model>> {
    Ok(goal::Entity::find()
        .filter(goal::Column::Status.eq("active"))
        .order_by_asc(goal::Column::Id)
        .all(db)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inputs::concern::NewConcern,
        services::concern,
        test_support::{date, test_db},
    };

    #[tokio::test]
    async fn set_creates_metric_goal_under_concern() {
        let db = test_db().await;
        let c = concern::open(
            &db,
            NewConcern {
                name: "Weight".into(),
                narrative: None,
                tags: vec![],
                opened_on: None,
            },
        )
        .await
        .unwrap();
        let g = set(
            &db,
            NewGoal {
                concern_id: Some(c.concern.id),
                title: "Reach 175 lbs".into(),
                description: None,
                metric_kind: Some("body_mass_lbs".into()),
                comparison: Some("at-most".into()),
                target_value: Some(175.0),
                target_high: None,
                target_date: Some(date("2026-12-31")),
            },
        )
        .await
        .unwrap();
        assert_eq!(g.status, "active");
        assert_eq!(list_active(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn set_validates_comparison_range_and_concern() {
        let db = test_db().await;
        // range needs target_high
        let res = set(
            &db,
            NewGoal {
                concern_id: None,
                title: "Sleep 7-8h".into(),
                description: None,
                metric_kind: Some("sleep_hours".into()),
                comparison: Some("range".into()),
                target_value: Some(7.0),
                target_high: None,
                target_date: None,
            },
        )
        .await;
        assert!(matches!(res, Err(DomainError::Invalid { .. })));
        // unknown comparison
        let res = set(
            &db,
            NewGoal {
                concern_id: None,
                title: "X".into(),
                description: None,
                metric_kind: Some("x".into()),
                comparison: Some("about".into()),
                target_value: Some(1.0),
                target_high: None,
                target_date: None,
            },
        )
        .await;
        assert!(matches!(res, Err(DomainError::BadRequest(_))));
        // dangling concern id
        let res = set(
            &db,
            NewGoal {
                concern_id: Some(999),
                title: "X".into(),
                description: None,
                metric_kind: None,
                comparison: None,
                target_value: None,
                target_high: None,
                target_date: None,
            },
        )
        .await;
        assert!(matches!(res, Err(DomainError::NotFound(_))));
    }

    #[tokio::test]
    async fn set_rejects_inverted_range() {
        let db = test_db().await;
        let res = set(
            &db,
            NewGoal {
                concern_id: None,
                title: "Sleep 7-8h".into(),
                description: None,
                metric_kind: Some("sleep_hours".into()),
                comparison: Some("range".into()),
                target_value: Some(8.0),
                target_high: Some(7.0),
                target_date: None,
            },
        )
        .await;
        assert!(matches!(res, Err(DomainError::Invalid { .. })));
    }

    #[tokio::test]
    async fn achieved_goal_leaves_active_list() {
        let db = test_db().await;
        let g = set(
            &db,
            NewGoal {
                concern_id: None,
                title: "Qualitative: no panic attacks".into(),
                description: None,
                metric_kind: None,
                comparison: None,
                target_value: None,
                target_high: None,
                target_date: None,
            },
        )
        .await
        .unwrap();
        update_status(&db, g.id, "achieved").await.unwrap();
        assert!(list_active(&db).await.unwrap().is_empty());
    }
}
