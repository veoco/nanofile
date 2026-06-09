use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("too many requests")]
    TooManyRequests,

    #[error("repo password required")]
    RepoPasswdRequired,

    #[error("repo password magic required")]
    RepoPasswdMagicRequired,

    #[error("internal error: {0}")]
    Internal(String),

    #[error("operation failed: {0}")]
    OperationFailed(String),

    #[error("repo deleted")]
    RepoDeleted,

    #[error("repo corrupted")]
    RepoCorrupted,

    /// 443 — storage quota exceeded (seafile wire protocol)
    #[error("storage quota exceeded")]
    QuotaExceeded,

    /// 446 — content blocks missing (seafile wire protocol)
    #[error("blocks missing")]
    BlockMissing,

    #[error("two factor auth token is missing")]
    TwoFactorRequired,

    #[error("invalid two factor auth token")]
    TwoFactorInvalid,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, json!({ "error": msg })),
            AppError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, json!({ "error": "Unauthorized" }))
            }
            AppError::Forbidden => (
                StatusCode::FORBIDDEN,
                json!({ "error": "Permission denied" }),
            ),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, json!({ "error": msg })),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, json!({ "error": msg })),
            AppError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "error": "Too many requests" }),
            ),
            AppError::RepoPasswdRequired => (
                StatusCode::from_u16(440).unwrap(),
                json!({ "error": "repo password is required" }),
            ),
            AppError::RepoPasswdMagicRequired => (
                StatusCode::from_u16(441).unwrap(),
                json!({ "error": "repo password magic is required" }),
            ),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": msg })),
            AppError::OperationFailed(msg) => {
                (StatusCode::from_u16(520).unwrap(), json!({ "error": msg }))
            }
            AppError::RepoDeleted => (
                StatusCode::from_u16(444).unwrap(),
                json!({ "error": "repo was deleted" }),
            ),
            AppError::RepoCorrupted => (
                StatusCode::from_u16(445).unwrap(),
                json!({ "error": "repo was corrupted" }),
            ),
            AppError::QuotaExceeded => (
                StatusCode::from_u16(443).unwrap(),
                json!({ "error": "Storage quota exceeded." }),
            ),
            AppError::BlockMissing => (
                StatusCode::from_u16(446).unwrap(),
                json!({ "error": "Blocks missing for uploaded files." }),
            ),
            // Seafile server wire format — sync client parses error_msg
            // to detect 2FA prompts; non_field_errors would be ignored.
            AppError::TwoFactorRequired => (
                StatusCode::UNAUTHORIZED,
                json!({ "error_msg": "Two factor auth token is missing.", "error_code": 401 }),
            ),
            AppError::TwoFactorInvalid => (
                StatusCode::UNAUTHORIZED,
                json!({ "error_msg": "Invalid two factor auth token.", "error_code": 401 }),
            ),
        };

        (status, axum::Json(body)).into_response()
    }
}

impl From<sea_orm::DbErr> for AppError {
    fn from(err: sea_orm::DbErr) -> Self {
        tracing::error!(?err, "database error");
        AppError::Internal(err.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        tracing::error!(?err, "IO error");
        AppError::Internal(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        tracing::error!(?err, "JSON serialization error");
        AppError::Internal(err.to_string())
    }
}

/// Add error-level logging when Internal (500) errors are returned.
/// This ensures all server-side failures are visible in the logs.
impl AppError {
    pub fn internal(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        tracing::error!("internal error: {}", msg);
        AppError::Internal(msg)
    }
}
