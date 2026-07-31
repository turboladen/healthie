//! `ApiError`: the HTTP boundary's error type. Maps the domain's
//! [`DomainError`](healthie_shared::error::DomainError) onto status codes and a
//! JSON `{ "error": ... }` body. Internal errors are logged and their detail is
//! withheld from the response.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Internal(msg) => {
                tracing::error!("internal error: {msg}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_owned(),
                )
            }
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

impl From<sea_orm::DbErr> for ApiError {
    fn from(err: sea_orm::DbErr) -> Self {
        ApiError::Internal(err.to_string())
    }
}

impl From<healthie_shared::error::DomainError> for ApiError {
    fn from(err: healthie_shared::error::DomainError) -> Self {
        use healthie_shared::error::DomainError as D;
        match err {
            D::NotFound(m) => ApiError::NotFound(m),
            D::Invalid { field, message } => ApiError::BadRequest(format!("{field}: {message}")),
            D::BadRequest(m) => ApiError::BadRequest(m),
            D::Db(e) => ApiError::Internal(e.to_string()),
            D::Internal(m) => ApiError::Internal(m),
        }
    }
}
