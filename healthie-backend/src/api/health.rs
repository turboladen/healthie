//! `GET /api/health`. Liveness with a real `SELECT 1` (a wedged data dir must
//! not read healthy), the crate version, and `build` — the git commit this
//! binary was built from, embedded by the build script.
//!
//! `build` exists for the deploy gate: a service that answers is not evidence
//! that the *new* binary is the one answering. Comparing the field against the
//! commit being deployed is what distinguishes a successful swap from a stop
//! that silently failed and left the old process serving.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use sea_orm::{ConnectionTrait, Statement};
use serde_json::json;

use crate::AppState;

pub async fn check(State(state): State<AppState>) -> impl IntoResponse {
    let backend = state.db.get_database_backend();
    match state
        .db
        .execute(Statement::from_string(backend, "SELECT 1"))
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "version": env!("CARGO_PKG_VERSION"),
                "build": env!("GIT_COMMIT"),
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "health: db check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "ok": false })),
            )
                .into_response()
        }
    }
}
