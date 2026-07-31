//! `GET /api/health`. Liveness with a real `SELECT 1` (a wedged data dir must
//! not read healthy) plus the build version. The git-SHA identity is added by
//! healthie-93n.

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
            Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") })),
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
