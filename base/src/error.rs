use thiserror::Error;

#[derive(Debug, Error)]
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

    #[error("request body too large")]
    ContentTooLarge,

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

impl AppError {
    pub fn internal(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        tracing::error!("internal error: {}", msg);
        AppError::Internal(msg)
    }
}

// ── Standard From impls (always available) ──────────────────────────────────

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

// ── From<sea_orm::DbErr> — only when the "with-sea-orm" feature is enabled ──

#[cfg(feature = "with-sea-orm")]
impl From<sea_orm::DbErr> for AppError {
    fn from(err: sea_orm::DbErr) -> Self {
        tracing::error!(?err, "database error");
        AppError::Internal(err.to_string())
    }
}

// ── IntoResponse + axum-related impls — only when "with-axum" is enabled ──

#[cfg(feature = "with-axum")]
mod into_response_impl {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use serde_json::json;

    use super::AppError;

    impl IntoResponse for AppError {
        fn into_response(self) -> Response {
            // The lock error is only ever produced by the sync protocol's
            // branch update, where the client regex-matches the body
            // (`"File (.+) is locked"`, `daemon/http-tx-mgr.c:261`) and puts the
            // captured group into the per-file sync-error notification. It must
            // therefore be plain text: an unanchored regex would otherwise
            // capture the JSON wrapper as part of the path (`"/a/b\"}"`).
            if let AppError::Locked(path) = &self {
                return (StatusCode::FORBIDDEN, format!("File {path} is locked")).into_response();
            }

            let (status, body) = match &self {
                AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, json!({ "error_msg": msg })),
                AppError::Unauthorized => (
                    StatusCode::UNAUTHORIZED,
                    json!({ "error_msg": "Unauthorized" }),
                ),
                AppError::Forbidden => (
                    StatusCode::FORBIDDEN,
                    json!({ "error_msg": "Permission denied" }),
                ),
                AppError::NotFound(msg) => (StatusCode::NOT_FOUND, json!({ "error_msg": msg })),
                AppError::Conflict(msg) => (StatusCode::CONFLICT, json!({ "error_msg": msg })),
                AppError::TooManyRequests => (
                    StatusCode::TOO_MANY_REQUESTS,
                    json!({ "error_msg": "Too many requests" }),
                ),
                AppError::ContentTooLarge => (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    json!({ "error_msg": "Request body too large" }),
                ),
                AppError::RepoPasswdRequired => (
                    StatusCode::from_u16(440).unwrap(),
                    json!({ "error_msg": "repo password is required" }),
                ),
                AppError::RepoPasswdMagicRequired => (
                    StatusCode::from_u16(441).unwrap(),
                    json!({ "error_msg": "repo password magic is required" }),
                ),
                AppError::Internal(_msg) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error_msg": "Internal server error" }),
                ),
                AppError::OperationFailed(msg) => (
                    StatusCode::from_u16(520).unwrap(),
                    json!({ "error_msg": msg }),
                ),
                AppError::RepoDeleted => (
                    StatusCode::from_u16(444).unwrap(),
                    json!({ "error_msg": "repo was deleted" }),
                ),
                AppError::RepoCorrupted => (
                    StatusCode::from_u16(445).unwrap(),
                    json!({ "error_msg": "repo was corrupted" }),
                ),
                AppError::QuotaExceeded => (
                    StatusCode::from_u16(443).unwrap(),
                    json!({ "error_msg": "Out of quota." }),
                ),
                AppError::BlockMissing => (
                    StatusCode::from_u16(446).unwrap(),
                    json!({ "error_msg": "Blocks missing for uploaded files." }),
                ),
                // Unreachable: `Locked` returns early above with the plain-text
                // body the daemon's error regex requires. Kept for
                // exhaustiveness (and as the pattern to reuse if that early
                // return is ever removed).
                AppError::Locked(path) => (
                    StatusCode::FORBIDDEN,
                    json!({ "error_msg": format!("File {} is locked", path) }),
                ),
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

    impl From<serde_urlencoded::de::Error> for AppError {
        fn from(err: serde_urlencoded::de::Error) -> Self {
            tracing::error!(?err, "form deserialization error");
            AppError::Internal(err.to_string())
        }
    }
}
