use axum::Json;

/// GET /upload_progress — returns current upload progress (always 100% for simplicity).
pub async fn upload_progress() -> Json<serde_json::Value> {
    Json(serde_json::json!({"progress": 100}))
}

/// GET /idx_progress — returns current indexing progress (always 100% for simplicity).
pub async fn idx_progress() -> Json<serde_json::Value> {
    Json(serde_json::json!({"progress": 100}))
}
