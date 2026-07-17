use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, ModelTrait,
    QueryFilter, QueryOrder,
};
use serde::Serialize;

use crate::{
    clock::{now_str, today_str},
    entities::{plan, plan_item, plan_item_outcome},
    error::{DomainError, DomainResult},
    inputs::plan::NewPlan,
    services::checkin,
};

pub const VALID_ITEM_KINDS: [&str; 2] = ["workout", "action"];
pub const VALID_OUTCOME_STATUSES: [&str; 3] = ["done", "skipped", "partial"];

#[derive(Debug, Serialize)]
pub struct ItemWithOutcome {
    pub item: plan_item::Model,
    pub outcome: Option<plan_item_outcome::Model>,
}

#[derive(Debug, Serialize)]
pub struct PlanWithItems {
    pub plan: plan::Model,
    pub items: Vec<ItemWithOutcome>,
}

pub async fn commit(db: &impl ConnectionTrait, input: NewPlan) -> DomainResult<PlanWithItems> {
    if input.items.is_empty() {
        return Err(DomainError::invalid("items", "a plan needs at least one item"));
    }
    for item in &input.items {
        if !VALID_ITEM_KINDS.contains(&item.kind.as_str()) {
            return Err(DomainError::BadRequest(format!(
                "Invalid item kind '{}'. Must be one of: {}",
                item.kind,
                VALID_ITEM_KINDS.join(", ")
            )));
        }
        if item.title.trim().is_empty() {
            return Err(DomainError::invalid("items.title", "must not be empty"));
        }
    }
    if let Some(cid) = input.checkin_id {
        checkin::require(db, cid).await?;
    }
    let plan_model = plan::ActiveModel {
        checkin_id: Set(input.checkin_id),
        starts_on: Set(input.starts_on.unwrap_or_else(today_str)),
        horizon_days: Set(input.horizon_days.unwrap_or(7)),
        guidance: Set(input.guidance),
        nutrition: Set(input.nutrition),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?;
    let mut items = Vec::with_capacity(input.items.len());
    for item in input.items {
        let m = plan_item::ActiveModel {
            plan_id: Set(plan_model.id),
            kind: Set(item.kind),
            title: Set(item.title),
            detail: Set(item.detail),
            scheduled_for: Set(item.scheduled_for),
            created_at: Set(now_str()),
            updated_at: Set(now_str()),
            ..Default::default()
        }
        .insert(db)
        .await?;
        items.push(ItemWithOutcome {
            item: m,
            outcome: None,
        });
    }
    Ok(PlanWithItems {
        plan: plan_model,
        items,
    })
}

pub async fn record_item_outcome(
    db: &impl ConnectionTrait,
    item_id: i32,
    status: &str,
    note: Option<String>,
) -> DomainResult<plan_item_outcome::Model> {
    if !VALID_OUTCOME_STATUSES.contains(&status) {
        return Err(DomainError::BadRequest(format!(
            "Invalid status '{status}'. Must be one of: {}",
            VALID_OUTCOME_STATUSES.join(", ")
        )));
    }
    let item = plan_item::Entity::find_by_id(item_id)
        .one(db)
        .await?
        .ok_or_else(|| DomainError::NotFound(format!("Plan item {item_id} not found")))?;
    // one outcome per item: replace any existing
    if let Some(existing) = plan_item_outcome::Entity::find()
        .filter(plan_item_outcome::Column::PlanItemId.eq(item.id))
        .one(db)
        .await?
    {
        existing.delete(db).await?;
    }
    Ok(plan_item_outcome::ActiveModel {
        plan_item_id: Set(item.id),
        status: Set(status.to_string()),
        note: Set(note),
        recorded_at: Set(now_str()),
        created_at: Set(now_str()),
        updated_at: Set(now_str()),
        ..Default::default()
    }
    .insert(db)
    .await?)
}

pub async fn latest(db: &impl ConnectionTrait) -> DomainResult<Option<PlanWithItems>> {
    let Some(p) = plan::Entity::find()
        .order_by_desc(plan::Column::StartsOn)
        .order_by_desc(plan::Column::Id)
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let raw_items = plan_item::Entity::find()
        .filter(plan_item::Column::PlanId.eq(p.id))
        .order_by_asc(plan_item::Column::Id)
        .all(db)
        .await?;
    let mut items = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        let outcome = plan_item_outcome::Entity::find()
            .filter(plan_item_outcome::Column::PlanItemId.eq(item.id))
            .one(db)
            .await?;
        items.push(ItemWithOutcome { item, outcome });
    }
    Ok(Some(PlanWithItems { plan: p, items }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inputs::plan::NewPlanItem,
        test_support::test_db,
    };

    fn pt_plan() -> NewPlan {
        NewPlan {
            checkin_id: None,
            starts_on: None,
            horizon_days: None,
            guidance: Some("Prioritize sleep; back is recovering.".into()),
            nutrition: Some("More fish, no late snacking.".into()),
            items: vec![
                NewPlanItem {
                    kind: "workout".into(),
                    title: "PT: bird-dogs 3x10".into(),
                    detail: None,
                    scheduled_for: Some("2026-07-17".into()),
                },
                NewPlanItem {
                    kind: "action".into(),
                    title: "Book colonoscopy".into(),
                    detail: Some("GP referral first".into()),
                    scheduled_for: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn commit_stores_plan_with_items_and_defaults() {
        let db = test_db().await;
        let p = commit(&db, pt_plan()).await.unwrap();
        assert_eq!(p.plan.horizon_days, 7);
        assert_eq!(p.items.len(), 2);
        assert!(p.items.iter().all(|i| i.outcome.is_none()));
    }

    #[tokio::test]
    async fn commit_rejects_empty_plan_and_bad_kind() {
        let db = test_db().await;
        let mut empty = pt_plan();
        empty.items.clear();
        assert!(matches!(
            commit(&db, empty).await,
            Err(DomainError::Invalid { .. })
        ));
        let mut bad = pt_plan();
        bad.items[0].kind = "chore".into();
        assert!(matches!(
            commit(&db, bad).await,
            Err(DomainError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn outcomes_record_and_replace() {
        let db = test_db().await;
        let p = commit(&db, pt_plan()).await.unwrap();
        let item_id = p.items[0].item.id;
        record_item_outcome(&db, item_id, "skipped", Some("back flared".into()))
            .await
            .unwrap();
        record_item_outcome(&db, item_id, "partial", None)
            .await
            .unwrap(); // replaces
        let latest = latest(&db).await.unwrap().unwrap();
        let outcome = latest
            .items
            .iter()
            .find(|i| i.item.id == item_id)
            .unwrap()
            .outcome
            .as_ref()
            .unwrap();
        assert_eq!(outcome.status, "partial");
    }
}
