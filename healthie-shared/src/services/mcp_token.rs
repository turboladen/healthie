//! Singleton MCP bearer-token lifecycle: provision (create or rotate),
//! constant-time verify, revoke. fewd's per-person scheme collapsed to one
//! operator row (id = 1). The plaintext leaves this module exactly once, in
//! [`ProvisionedToken`]; it is never logged and never stored.

use argon2::{
    Argon2,
    password_hash::{
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
        rand_core::{OsRng, RngCore},
    },
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, EntityTrait};

use crate::{
    clock,
    entities::mcp_token,
    error::{DomainError, DomainResult},
};

/// The one-time provision result. `plaintext` is shown to the operator once;
/// only its argon2id hash is stored.
pub struct ProvisionedToken {
    pub plaintext: String,
    pub fingerprint: String,
}

/// Manual impl so `{:?}` can never leak the live plaintext into logs or
/// panic messages; only the (deliberately cleartext) fingerprint is shown.
impl std::fmt::Debug for ProvisionedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionedToken")
            .field("plaintext", &"<redacted>")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

/// Create or rotate the operator token. Returns the plaintext exactly once.
///
/// # Errors
/// `DomainError::Internal` if argon2 hashing fails; `DomainError::Db` on
/// database errors.
pub async fn provision(db: &impl ConnectionTrait) -> DomainResult<ProvisionedToken> {
    let plaintext = generate_plaintext();
    let fingerprint = fingerprint_of(&plaintext);
    let hash = hash_plaintext(&plaintext)
        .map_err(|err| DomainError::Internal(format!("token hashing failed: {err}")))?;
    let now = clock::now();

    // Branch insert/update explicitly (never .save() with a Set PK).
    match mcp_token::Entity::find_by_id(1).one(db).await? {
        Some(existing) => {
            let mut active: mcp_token::ActiveModel = existing.into();
            active.token_hash = Set(hash);
            active.fingerprint = Set(fingerprint.clone());
            active.updated_at = Set(now);
            active.update(db).await?;
        }
        None => {
            mcp_token::ActiveModel {
                id: Set(1),
                token_hash: Set(hash),
                fingerprint: Set(fingerprint.clone()),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(db)
            .await?;
        }
    }
    Ok(ProvisionedToken {
        plaintext,
        fingerprint,
    })
}

/// Verify a presented bearer token. `Some(fingerprint)` on match; `None` when
/// no token is provisioned or the token doesn't match. Comparison happens via
/// argon2's `verify_password` (constant-time internally).
///
/// # Errors
/// `DomainError::Db` on database errors. A stored hash that fails to parse is
/// treated as no-match (logged), not an error — auth must fail closed.
pub async fn verify(db: &impl ConnectionTrait, presented: &str) -> DomainResult<Option<String>> {
    let Some(row) = mcp_token::Entity::find_by_id(1).one(db).await? else {
        return Ok(None);
    };
    let parsed = match PasswordHash::new(&row.token_hash) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!(
                ?err,
                "mcp_token: stored hash did not parse — failing closed"
            );
            return Ok(None);
        }
    };
    if Argon2::default()
        .verify_password(presented.as_bytes(), &parsed)
        .is_ok()
    {
        Ok(Some(row.fingerprint))
    } else {
        Ok(None)
    }
}

/// Delete the token row. Idempotent — revoking when nothing is provisioned
/// succeeds.
///
/// # Errors
/// `DomainError::Db` on database errors.
pub async fn revoke(db: &impl ConnectionTrait) -> DomainResult<()> {
    mcp_token::Entity::delete_by_id(1).exec(db).await?;
    Ok(())
}

fn generate_plaintext() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn fingerprint_of(plaintext: &str) -> String {
    plaintext.chars().take(8).collect()
}

fn hash_plaintext(plaintext: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)?
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_db;

    #[tokio::test]
    async fn provision_verify_round_trip() {
        let db = test_db().await;
        let issued = provision(&db).await.expect("provision");
        assert_eq!(issued.plaintext.len(), 43, "32 bytes base64url-no-pad");
        assert_eq!(issued.fingerprint.len(), 8);
        assert!(issued.plaintext.starts_with(&issued.fingerprint));

        let got = verify(&db, &issued.plaintext).await.expect("verify");
        assert_eq!(got.as_deref(), Some(issued.fingerprint.as_str()));
    }

    #[tokio::test]
    async fn stored_hash_is_argon2id_not_plaintext() {
        let db = test_db().await;
        let issued = provision(&db).await.expect("provision");
        let row = crate::entities::mcp_token::Entity::find_by_id(1)
            .one(&db)
            .await
            .expect("query")
            .expect("row");
        assert!(row.token_hash.starts_with("$argon2id$"));
        assert_ne!(row.token_hash, issued.plaintext);
        assert!(!row.token_hash.contains(&issued.plaintext));
    }

    #[tokio::test]
    async fn wrong_token_verifies_to_none() {
        let db = test_db().await;
        provision(&db).await.expect("provision");
        let wrong = "A".repeat(43);
        assert_eq!(verify(&db, &wrong).await.expect("verify"), None);
    }

    #[tokio::test]
    async fn verify_with_no_token_provisioned_is_none() {
        let db = test_db().await;
        assert_eq!(verify(&db, "anything").await.expect("verify"), None);
    }

    #[tokio::test]
    async fn rotation_invalidates_previous_plaintext() {
        let db = test_db().await;
        let first = provision(&db).await.expect("first");
        let second = provision(&db).await.expect("second");
        assert_ne!(first.plaintext, second.plaintext);
        assert_eq!(verify(&db, &first.plaintext).await.expect("verify"), None);
        assert!(
            verify(&db, &second.plaintext)
                .await
                .expect("verify")
                .is_some()
        );
    }

    #[tokio::test]
    async fn revoke_clears_token_and_is_idempotent() {
        let db = test_db().await;
        let issued = provision(&db).await.expect("provision");
        revoke(&db).await.expect("revoke");
        assert_eq!(verify(&db, &issued.plaintext).await.expect("verify"), None);
        revoke(&db).await.expect("revoke again (idempotent)");
    }

    #[test]
    fn debug_format_redacts_plaintext() {
        let token = ProvisionedToken {
            plaintext: "secret-token-value".to_owned(),
            fingerprint: "abcd1234".to_owned(),
        };
        let debug = format!("{token:?}");
        assert!(
            !debug.contains("secret-token-value"),
            "Debug must not leak the plaintext: {debug}"
        );
        assert!(debug.contains("<redacted>"));
        assert!(
            debug.contains("abcd1234"),
            "fingerprint stays visible: {debug}"
        );
    }

    #[tokio::test]
    async fn model_serialization_omits_token_hash() {
        let db = test_db().await;
        provision(&db).await.expect("provision");
        let row = mcp_token::Entity::find_by_id(1)
            .one(&db)
            .await
            .expect("query")
            .expect("row");
        let json = serde_json::to_string(&row).expect("serialize");
        assert!(
            !json.contains("token_hash"),
            "hash key must not serialize: {json}"
        );
        assert!(
            !json.contains("$argon2id$"),
            "hash value must not serialize: {json}"
        );
        assert!(
            json.contains(&row.fingerprint),
            "fingerprint serializes on purpose"
        );
    }
}
