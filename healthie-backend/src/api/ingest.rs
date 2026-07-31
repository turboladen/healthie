//! `POST /ingest/hae`. The real ingest (→ `metrics::ingest_hae` → 204) and the
//! `require_ingest_token` bearer middleware land in Task 9; this scaffold returns
//! a bare 204 with NO auth layer, so no intermediate commit gates health.

use axum::http::StatusCode;

// filled in Task 9 (metrics::ingest_hae → 204; require_ingest_token middleware)
pub async fn hae() -> StatusCode {
    StatusCode::NO_CONTENT
}
