//! Bearer-token auth for the MCP surface (healthie-1ci). Single-user, but
//! REQUIRED: this server is built to be exposed over Tailscale.
//!
//! Invariants: the presented token is NEVER logged, on any path. Auth failures
//! are 401 with a JSON body; DB failures during lookup are 500 without detail.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::{
    headers::{Authorization, authorization::Bearer},
    typed_header::TypedHeader,
};
use healthie_shared::services::mcp_token;
use rmcp::{
    ErrorData as McpError,
    service::{RequestContext, RoleServer},
};
use sea_orm::DatabaseConnection;
use serde_json::json;

/// The verified operator identity, injected into request extensions by
/// [`require_mcp_token`] and read via [`authenticated_operator`] — the ONE
/// grep-able place any future per-request authz goes through.
#[derive(Clone, Debug)]
pub struct AuthenticatedOperator {
    pub fingerprint: String,
}

/// Gate every request behind `Authorization: Bearer <mcp-token>`.
///
/// Header parsing (scheme case, whitespace) is delegated to `axum_extra`'s
/// `TypedHeader<Authorization<Bearer>>`; verification is the constant-time
/// argon2id check in [`mcp_token::verify`]. On success the resolved
/// [`AuthenticatedOperator`] rides the request into the tool handlers via
/// request extensions. The presented token is never logged, even on failure.
pub(crate) async fn require_mcp_token(
    State(db): State<DatabaseConnection>,
    bearer: Option<TypedHeader<Authorization<Bearer>>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let Some(TypedHeader(auth)) = bearer else {
        return unauthorized("missing Authorization: Bearer <mcp-token>");
    };
    let token = auth.token().trim();
    if token.is_empty() {
        return unauthorized("missing Authorization: Bearer <mcp-token>");
    }
    match mcp_token::verify(&db, token).await {
        Ok(Some(fingerprint)) => {
            req.extensions_mut()
                .insert(AuthenticatedOperator { fingerprint });
            next.run(req).await
        }
        Ok(None) => unauthorized("invalid or revoked token"),
        Err(err) => {
            tracing::error!(?err, "MCP auth: token lookup failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "auth lookup failed")
        }
    }
}

/// Read the operator identity a tool's `RequestContext` carries. Fails loudly
/// (opaque internal error) if the router was mounted without the middleware —
/// defense in depth against a misconfigured M2 mount. Any future authz check
/// goes through this one grep-able site.
///
/// # Errors
/// [`McpError::internal_error`] if the HTTP request parts or the
/// [`AuthenticatedOperator`] extension are missing — both indicate the auth
/// middleware was not in the request path.
pub(crate) fn authenticated_operator(
    context: &RequestContext<RoleServer>,
) -> Result<AuthenticatedOperator, McpError> {
    let parts = context
        .extensions
        .get::<axum::http::request::Parts>()
        .ok_or_else(|| {
            tracing::error!("MCP auth: missing http request parts in tool context");
            McpError::internal_error("missing http request parts", None)
        })?;
    parts
        .extensions
        .get::<AuthenticatedOperator>()
        .cloned()
        .ok_or_else(|| {
            tracing::error!("MCP auth: missing AuthenticatedOperator extension");
            McpError::internal_error("missing authenticated operator", None)
        })
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
