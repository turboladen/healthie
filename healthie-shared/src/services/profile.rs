use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, EntityTrait};

use crate::{
    clock::now,
    entities::profile,
    error::{DomainError, DomainResult},
    inputs::profile::UpdateProfile,
};

pub const VALID_SEXES: [&str; 2] = ["male", "female"];

pub async fn get(db: &impl ConnectionTrait) -> DomainResult<Option<profile::Model>> {
    Ok(profile::Entity::find_by_id(1).one(db).await?)
}

pub async fn upsert(
    db: &impl ConnectionTrait,
    input: UpdateProfile,
) -> DomainResult<profile::Model> {
    if let Some(Some(sex)) = &input.sex
        && !VALID_SEXES.contains(&sex.as_str())
    {
        return Err(DomainError::BadRequest(format!(
            "Invalid sex '{sex}'. Must be one of: {}",
            VALID_SEXES.join(", ")
        )));
    }

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
    use crate::test_support::{date, test_db};

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
                sex: Some(Some("male".into())),
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

    #[tokio::test]
    async fn upsert_rejects_bad_sex() {
        let db = test_db().await;
        assert!(matches!(
            upsert(
                &db,
                UpdateProfile {
                    sex: Some(Some("yes".into())),
                    ..Default::default()
                }
            )
            .await,
            Err(DomainError::BadRequest(_))
        ));
    }
}
