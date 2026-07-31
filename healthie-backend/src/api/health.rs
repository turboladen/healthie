//! `GET /api/health`. Liveness probe. The real `SELECT 1` + version body lands
//! in Task 8; this scaffold returns a bare 200 so the router compiles.

use axum::http::StatusCode;

// filled in Task 8 (real SELECT 1 + version)
pub async fn check() -> StatusCode {
    StatusCode::OK
}
