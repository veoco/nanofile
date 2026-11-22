//! One body parser for the "JSON or form-encoded" request convention.
//!
//! Seafile clients disagree: the desktop client posts form data for link
//! creation and trash restore, while web and Android send JSON. `JsonOrForm`
//! normalizes both to a `serde_json::Value` for handlers that need ad-hoc
//! field access; [`parse_json_or_form`] does the same for a typed body.

use axum::{
    body::to_bytes,
    extract::{FromRequest, Request},
    http::header,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;

use base::error::AppError;

/// Accept a JSON object or an `application/x-www-form-urlencoded` body; the
/// desktop client posts form-encoded link data while web/Android send JSON.
pub struct JsonOrForm(pub Value);

impl<S> FromRequest<S> for JsonOrForm
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let bytes = to_bytes(req.into_body(), 8 * 1024 * 1024)
            .await
            .map_err(|e| AppError::BadRequest(format!("failed to read body: {e}")))?;

        if content_type.starts_with("application/x-www-form-urlencoded") {
            let map = serde_urlencoded::from_bytes::<HashMap<String, String>>(&bytes)
                .map_err(|e| AppError::BadRequest(format!("invalid form body: {e}")))?;
            Ok(JsonOrForm(serde_json::to_value(map).unwrap_or_default()))
        } else {
            serde_json::from_slice::<Value>(&bytes)
                .map(JsonOrForm)
                .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))
        }
    }
}

/// Parse a small body as JSON first, then URL-encoded form.
pub fn parse_json_or_form<T: DeserializeOwned>(body: &[u8]) -> Result<T, AppError> {
    if let Ok(parsed) = serde_json::from_slice::<T>(body) {
        return Ok(parsed);
    }
    serde_urlencoded::from_bytes::<T>(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))
}
