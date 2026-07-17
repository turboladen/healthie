use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::{
    clock::now,
    entities::goal::{self, GoalComparison, GoalStatus},
    error::{DomainError, DomainResult},
    inputs::goal::NewGoal,
    services::concern,
};

/// Loads a goal by id.
///
/// # Errors
/// `DomainError::NotFound` if no goal has id `id`; `DomainError::Db` on database
/// failure.
pub async fn require(db: &impl ConnectionTrait, id: i32) -> DomainResult<goal::Model> {
    goal::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Goal {id} not found")))
}

/// Creates a goal, optionally under a concern.
///
/// # Errors
/// `DomainError::Invalid` if `title` is empty, or if `comparison` is set without
/// a `target_value`, or a `range` comparison lacks/inverts `target_high`;
/// `DomainError::NotFound` if `concern_id` refers to no concern;
/// `DomainError::Db` on database failure.
pub async fn set(db: &impl ConnectionTrait, input: NewGoal) -> DomainResult<goal::Model> {
    if input.title.trim().is_empty() {
        return Err(DomainError::invalid("title", "must not be empty"));
    }
    if let Some(cid) = input.concern_id {
        concern::require(db, cid).await?;
    }
    if let Some(cmp) = input.comparison {
        if input.target_value.is_none() {
            return Err(DomainError::invalid(
                "target_value",
                "required when comparison is set",
            ));
        }
        if cmp == GoalComparison::Range && input.target_high.is_none() {
            return Err(DomainError::invalid(
                "target_high",
                "required when comparison is 'range'",
            ));
        }
        if cmp == GoalComparison::Range
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
        status: Set(GoalStatus::Active),
        created_at: Set(now()),
        updated_at: Set(now()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

/// Sets a goal's status.
///
/// # Errors
/// `DomainError::NotFound` if no goal has id `id`; `DomainError::Db` on database
/// failure.
pub async fn update_status(
    db: &impl ConnectionTrait,
    id: i32,
    status: GoalStatus,
) -> DomainResult<goal::Model> {
    let mut active: goal::ActiveModel = require(db, id).await?.into();
    active.status = Set(status);
    active.updated_at = Set(now());
    Ok(active.update(db).await?)
}

/// Lists every active goal.
///
/// # Errors
/// `DomainError::Db` on database failure.
pub async fn list_active(db: &impl ConnectionTrait) -> DomainResult<Vec<goal::Model>> {
    Ok(goal::Entity::find()
        .filter(goal::Column::Status.eq(GoalStatus::Active))
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
                comparison: Some(GoalComparison::AtMost),
                target_value: Some(175.0),
                target_high: None,
                target_date: Some(date("2026-12-31")),
            },
        )
        .await
        .unwrap();
        assert_eq!(g.status, GoalStatus::Active);
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
                comparison: Some(GoalComparison::Range),
                target_value: Some(7.0),
                target_high: None,
                target_date: None,
            },
        )
        .await;
        assert!(matches!(res, Err(DomainError::Invalid { .. })));
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
                comparison: Some(GoalComparison::Range),
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
        update_status(&db, g.id, GoalStatus::Achieved)
            .await
            .unwrap();
        assert!(list_active(&db).await.unwrap().is_empty());
    }
}
