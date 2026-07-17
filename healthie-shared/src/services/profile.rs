use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, EntityTrait};

use crate::{clock::now, entities::profile, error::DomainResult, inputs::profile::UpdateProfile};

pub async fn get(db: &impl ConnectionTrait) -> DomainResult<Option<profile::Model>> {
    Ok(profile::Entity::find_by_id(1).one(db).await?)
}

pub async fn upsert(
    db: &impl ConnectionTrait,
    input: UpdateProfile,
) -> DomainResult<profile::Model> {
    // Branch insert/update explicitly. Do NOT use `.save()`: with the PK Set(1) it
    // always takes the UPDATE path, which fails (RecordNotUpdated) on first call.
    let existing = get(db).await?;
    let is_insert = existing.is_none();
    let mut active: profile::ActiveModel = match existing {
        Some(m) => m.into(),
        None => profile::ActiveModel {
            id: Set(1),
            created_at: Set(now()),
            ..Default::default()
        },
    };
    if let Some(v) = input.date_of_birth {
        active.date_of_birth = Set(v);
    }
    if let Some(v) = input.sex {
        active.sex = Set(v);
    }
    if let Some(v) = input.height_cm {
        active.height_cm = Set(v);
    }
    if let Some(v) = input.notes {
        active.notes = Set(v);
    }
    active.updated_at = Set(now());
    if is_insert {
        Ok(active.insert(db).await?)
    } else {
        Ok(active.update(db).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::profile::Sex,
        test_support::{date, test_db},
    };

    #[tokio::test]
    async fn get_returns_none_before_first_upsert() {
        let db = test_db().await;
        assert!(get(&db).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_creates_then_updates_singleton() {
        let db = test_db().await;
        let p = upsert(
            &db,
            UpdateProfile {
                date_of_birth: Some(Some(date("1981-03-02"))),
                sex: Some(Some(Sex::Male)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(p.id, 1);
        let p2 = upsert(
            &db,
            UpdateProfile {
                height_cm: Some(Some(180)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(p2.id, 1);
        assert_eq!(p2.date_of_birth, Some(date("1981-03-02"))); // preserved
        assert_eq!(p2.height_cm, Some(180));
    }
}
