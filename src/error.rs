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

    /// 403 — file is locked by another user (seafile wire protocol)
    /// The daemon parses the body with regex "File (.+) is locked"
    /// and emits SYNC_ERROR_ID_FILE_LOCKED.
    #[error("file is locked: {0}")]
    Locked(String),

    #[error("two factor auth token is missing")]
    TwoFactorRequired,

    #[error("invalid two factor auth token")]
    TwoFactorInvalid,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, json!({ "error_msg":msg })),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                json!({ "error_msg":"Unauthorized" }),
            ),
            AppError::Forbidden => (
                StatusCode::FORBIDDEN,
                json!({ "error_msg":"Permission denied" }),
            ),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, json!({ "error_msg":msg })),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, json!({ "error_msg":msg })),
            AppError::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "error_msg":"Too many requests" }),
            ),
            AppError::RepoPasswdRequired => (
                StatusCode::from_u16(440).unwrap(),
                json!({ "error_msg":"repo password is required" }),
            ),
            AppError::RepoPasswdMagicRequired => (
                StatusCode::from_u16(441).unwrap(),
                json!({ "error_msg":"repo password magic is required" }),
            ),
            AppError::Internal(_msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error_msg":"Internal server error" }),
            ),
            AppError::OperationFailed(msg) => (
                StatusCode::from_u16(520).unwrap(),
                json!({ "error_msg":msg }),
            ),
            AppError::RepoDeleted => (
                StatusCode::from_u16(444).unwrap(),
                json!({ "error_msg":"repo was deleted" }),
            ),
            AppError::RepoCorrupted => (
                StatusCode::from_u16(445).unwrap(),
                json!({ "error_msg":"repo was corrupted" }),
            ),
            AppError::QuotaExceeded => (
                StatusCode::from_u16(443).unwrap(),
                json!({ "error_msg":"Storage quota exceeded." }),
            ),
            AppError::BlockMissing => (
                StatusCode::from_u16(446).unwrap(),
                json!({ "error_msg":"Blocks missing for uploaded files." }),
            ),
            AppError::Locked(path) => (
                StatusCode::FORBIDDEN,
                json!({ "error_msg":format!("File {} is locked", path) }),
            ),
            // Seahub-compatible 2FA error format: 400 + non_field_errors.
            // The login handler in auth.rs adds the X-Seafile-OTP header directly;
            // these serve as fallbacks for other call sites (e.g. two_factor.rs).
            AppError::TwoFactorRequired => (
                StatusCode::BAD_REQUEST,
                json!({"non_field_errors": ["Two factor auth token is missing."]}),
            ),
            AppError::TwoFactorInvalid => (
                StatusCode::BAD_REQUEST,
                json!({"non_field_errors": ["Invalid two factor auth token."]}),
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

impl From<serde_urlencoded::de::Error> for AppError {
    fn from(err: serde_urlencoded::de::Error) -> Self {
        tracing::error!(?err, "form deserialization error");
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
