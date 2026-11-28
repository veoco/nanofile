use axum::Json;

/// GET /api/transfers/upload-progress/ — current upload progress. Uploads are
/// synchronous per request, so the answer is always 100%.
pub async fn upload_progress() -> Json<serde_json::Value> {
    Json(serde_json::json!({"progress": 100}))
}

/// GET /api/transfers/index-progress/ — the indexing counterpart of the upload
/// progress shim; always 100% when full-text indexing is enabled.
pub async fn idx_progress() -> Json<serde_json::Value> {
    Json(serde_json::json!({"progress": 100}))
}
