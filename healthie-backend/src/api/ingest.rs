//! `POST /ingest/hae`: bearer(ingest)-gated Health Auto Export intake. The
//! handler is a thin adapter — one call into `metrics::ingest_hae` (all
//! validation and persistence live there) — returning 204. The bearer gate
//! mirrors healthie-mcp's `require_mcp_token`: 401 JSON on missing/invalid, 500
//! on lookup error, and the presented token is NEVER logged on any path.

use axum::{
    body::Body,
    extract::{Json, State},
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::{
    headers::{Authorization, authorization::Bearer},
    typed_header::TypedHeader,
};
use healthie_shared::{
    entities::auth_token::TokenKind,
    services::{auth_token, metrics},
};
use serde_json::json;

use crate::{AppState, api::error::ApiError};

/// Gate `/ingest/hae` behind `Authorization: Bearer <ingest-token>`. Header
/// parsing is delegated to `axum_extra`; verification is the constant-time
/// argon2id check in [`auth_token::verify`] scoped to [`TokenKind::Ingest`], so
/// an MCP token can never drive ingest. The token is never logged, even on
/// failure.
pub async fn require_ingest_token(
    State(state): State<AppState>,
    bearer: Option<TypedHeader<Authorization<Bearer>>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(TypedHeader(auth)) = bearer else {
        return unauthorized("missing Authorization: Bearer <ingest-token>");
    };
    let token = auth.token().trim();
    if token.is_empty() {
        return unauthorized("missing Authorization: Bearer <ingest-token>");
    }
    match auth_token::verify(&state.db, TokenKind::Ingest, token).await {
        Ok(Some(_fingerprint)) => next.run(request).await,
        Ok(None) => unauthorized("invalid or revoked token"),
        Err(err) => {
            tracing::error!(?err, "ingest auth: token lookup failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "auth lookup failed")
        }
    }
}

/// Ingest one HAE payload → 204. The [`IngestReport`](metrics::IngestReport) is
/// logged; the quarantine rows are the durable record of unknown metrics.
///
/// # Errors
/// Returns [`ApiError::Internal`] if the ingest transaction fails (surfaced from
/// [`metrics::ingest_hae`]).
pub async fn hae(
    State(state): State<AppState>,
    Json(payload): Json<metrics::HaePayload>,
) -> Result<StatusCode, ApiError> {
    let report = metrics::ingest_hae(&state.db, payload).await?;
    // A push that refused something is the one an operator needs to see. This
    // runs unattended every day, so an `info!` line indistinguishable from the
    // 364 ordinary ones would not be read — and a metric refused on EVERY push
    // means the live feed changed shape and rows that used to land are now
    // being held instead. That is the signal `warn` exists for.
    if report.refused.is_empty() && report.bounds_cleared == 0 {
        tracing::info!(
            ingested = report.ingested,
            quarantined = ?report.quarantined,
            range = ?report.date_range,
            quarantine_retired = report.quarantine_retired,
            "hae ingest",
        );
    } else {
        tracing::warn!(
            ingested = report.ingested,
            quarantined = ?report.quarantined,
            range = ?report.date_range,
            bounds_cleared = report.bounds_cleared,
            quarantine_retired = report.quarantine_retired,
            refused = ?report.refused,
            // Deliberately not "refused points": a cleared bound still stores
            // its row, and `Refusal::stored` says which happened. Claiming
            // nothing landed would be worse than saying less.
            "hae ingest held some readings back — see refused; the points are in quarantined_metric",
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

fn unauthorized(message: &str) -> Response {
    error_response(StatusCode::UNAUTHORIZED, message)
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        json!({ "error": message }).to_string(),
    )
        .into_response()
}
