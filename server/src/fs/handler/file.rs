use crate::common::util::extract_multipart_field;
use crate::sanitize::safe_normalize_path;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Request},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::fs::service::file::{self as file_svc, FileService};

// Re-export pub(crate) rename function for create_file_v21 use
pub(crate) use file_svc::rename_file_entry;

#[derive(Deserialize)]
pub struct FileQuery {
    pub p: Option<String>,
    pub reuse: Option<i32>,
}

pub fn file_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/file/",
            axum::routing::get(download_file)
                .post(file_post_handler)
                .put(lock_file_via_api_handler)
                .delete(delete_file),
        )
        .route("/{repo_id}/file/rename/", axum::routing::post(rename_file))
        .route("/{repo_id}/file/move/", axum::routing::post(move_file))
        .route("/{repo_id}/file/detail/", axum::routing::get(file_detail))
}

pub async fn download_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Response, AppError> {
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = safe_normalize_path(&query.p.unwrap_or_else(|| "/".to_string()))
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );

    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let (file_fs_id, url) = svc
        .get_download_info(&repo_id, &path, auth.user_id, &auth.email, host_header)
        .await?;

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        HeaderName::from_static("oid"),
        HeaderValue::from_str(&file_fs_id).unwrap_or_else(|_| {
            HeaderValue::from_static("0000000000000000000000000000000000000000")
        }),
    );

    Ok((resp_headers, Json(url)).into_response())
}

pub async fn file_post_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let content_type = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );

    if content_type.starts_with("multipart/form-data") {
        // Check for rename operation in multipart body
        if let Some(op) = extract_multipart_field(&bytes, "operation")
            && op == "rename"
        {
            let newname = extract_multipart_field(&bytes, "newname")
                .ok_or_else(|| AppError::BadRequest("newname required".into()))?;
            let path = safe_normalize_path(&query.p.unwrap_or_default())
                .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
            rename_file_entry(
                state.db.as_ref(),
                &repo_id,
                &path,
                &newname,
                &auth.email,
                auth.user_id,
            )
            .await?;
            Ok(Json(serde_json::Value::String("success".to_string())))
        } else {
            // Parse multipart fields into UploadedFile
            let mut upload = crate::fs::service::file::UploadedFile {
                file_name: String::new(),
                file_data: Vec::new(),
                parent_dir: "/".to_string(),
                replace: false,
            };

            use futures::stream;
            use multer::Multipart as MulterMultipart;
            let ct = parts
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let boundary = multer::parse_boundary(ct)
                .map_err(|e| AppError::Internal(format!("invalid multipart boundary: {e}")))?;
            let stream = stream::once(async { Ok::<_, std::convert::Infallible>(bytes.clone()) });
            let mut mp = MulterMultipart::new(stream, boundary);
            while let Some(field) = mp
                .next_field()
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?
            {
                let field_name = field.name().unwrap_or_default().to_string();
                match field_name.as_str() {
                    "file" => {
                        upload.file_name = field.file_name().unwrap_or_default().to_string();
                        upload.file_data = field
                            .bytes()
                            .await
                            .map_err(|e| AppError::Internal(e.to_string()))?
                            .to_vec();
                    }
                    "parent_dir" => {
                        let data = field
                            .bytes()
                            .await
                            .map_err(|e| AppError::Internal(e.to_string()))?;
                        upload.parent_dir = String::from_utf8(data.to_vec()).unwrap_or_default();
                    }
                    "replace" => {
                        let data = field
                            .bytes()
                            .await
                            .map_err(|e| AppError::Internal(e.to_string()))?;
                        let val = String::from_utf8(data.to_vec()).unwrap_or_default();
                        upload.replace = val.trim() == "1" || val.trim() == "true";
                    }
                    _ => {}
                }
            }

            svc.upload_file(&repo_id, upload, &auth.email, auth.user_id)
                .await?;
            Ok(Json(serde_json::json!({"success": true})))
        }
    } else {
        let form: HashMap<String, String> = serde_urlencoded::from_bytes(&bytes)
            .map_err(|_| AppError::BadRequest("invalid form data".into()))?;
        let path = safe_normalize_path(&query.p.unwrap_or_default())
            .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

        match form.get("operation").map(|s| s.as_str()) {
            Some("rename") => {
                let newname = form
                    .get("newname")
                    .ok_or_else(|| AppError::BadRequest("newname required".into()))?;
                svc.rename_file(&repo_id, &path, newname, &auth.email, auth.user_id)
                    .await?;
                Ok(Json(serde_json::json!({"success": true})))
            }
            Some("move") => {
                let _dst_repo = form
                    .get("dst_repo")
                    .ok_or_else(|| AppError::BadRequest("dst_repo required".into()))?;
                let dst_dir = form.get("dst_dir").map(|s| s.as_str()).unwrap_or("/");
                svc.move_file(&repo_id, &path, dst_dir, &auth.email, auth.user_id)
                    .await?;
                Ok(Json(serde_json::json!({"success": true})))
            }
            _ => Err(AppError::BadRequest("unknown operation".into())),
        }
    }
}

pub async fn delete_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<(), AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = safe_normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path is required".into()))?,
    )
    .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );
    svc.delete_file(&repo_id, &path, &auth.email, auth.user_id)
        .await
}

#[derive(Deserialize)]
pub struct MoveRequest {
    pub repo_id: String,
    pub p: String,
    pub new_parent_dir: String,
}

pub async fn move_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MoveRequest>,
) -> Result<(), AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    let path = safe_normalize_path(&req.p)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );
    svc.move_file(
        &req.repo_id,
        &path,
        &req.new_parent_dir,
        &auth.email,
        auth.user_id,
    )
    .await
}

#[derive(Deserialize)]
pub struct RenameRequest {
    pub repo_id: String,
    pub p: String,
    pub new_name: String,
}

pub async fn rename_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameRequest>,
) -> Result<(), AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    let path = safe_normalize_path(&req.p)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );
    svc.rename_file(
        &req.repo_id,
        &path,
        &req.new_name,
        &auth.email,
        auth.user_id,
    )
    .await
}

pub async fn file_detail(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = safe_normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path is required".into()))?,
    )
    .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );
    let result = svc.file_detail(&repo_id, &path).await?;

    Ok(Json(result))
}

pub async fn lock_file_via_api_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&bytes)
        .map_err(|_| AppError::BadRequest("invalid form data".into()))?;

    let operation = form
        .get("operation")
        .map(|s| s.as_str())
        .ok_or_else(|| AppError::BadRequest("operation required".into()))?;
    let path = safe_normalize_path(&query.p.unwrap_or_default())
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let db = state.db.as_ref();

    crate::storage::check_repo_write_permission(db, &repo_id, auth.user_id).await?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );
    svc.lock_file(&repo_id, &path, operation, &auth.email, auth.user_id)
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

#[derive(Deserialize)]
pub struct CreateFileRequest {
    pub p: Option<String>,
}

pub async fn create_file_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let content_type = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let path: String = if content_type.contains("json") {
        let r = serde_json::from_slice::<CreateFileRequest>(&bytes)?;
        r.p.ok_or_else(|| AppError::BadRequest("path (p) required".into()))?
    } else {
        query
            .get("p")
            .cloned()
            .or_else(|| extract_multipart_field(&bytes, "p"))
            .ok_or_else(|| AppError::BadRequest("path (p) required".into()))?
    };
    let path = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    // Check for rename operation in multipart body
    if let Some(op) = extract_multipart_field(&bytes, "operation")
        && op == "rename"
    {
        let newname = extract_multipart_field(&bytes, "newname")
            .ok_or_else(|| AppError::BadRequest("newname required".into()))?;
        self::rename_file_entry(
            state.db.as_ref(),
            &repo_id,
            &path,
            &newname,
            &auth.email,
            auth.user_id,
        )
        .await?;
        return Ok(Json(serde_json::json!({"success": true})));
    }

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );
    svc.create_empty_file(&repo_id, &path, &auth.email, auth.user_id)
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

#[derive(Deserialize)]
pub struct UploadedBytesQuery {
    pub file_name: Option<String>,
    pub parent_dir: Option<String>,
    pub blockids: Option<String>,
}

pub async fn file_uploaded_bytes(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<UploadedBytesQuery>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    if query.file_name.as_deref().is_none_or(|s| s.is_empty()) {
        return Err(AppError::BadRequest("file_name invalid.".into()));
    }
    if query.parent_dir.as_deref().is_none_or(|s| s.is_empty()) {
        return Err(AppError::BadRequest("parent_dir invalid.".into()));
    }

    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );
    let mut uploaded_bytes = svc.check_uploaded_bytes(query.blockids.as_deref()).await;

    // For resumable upload: check if a temp file already exists and return
    // the actual byte offset.  This takes precedence over the block-count
    // approach (which is used by the block-level resume protocol).
    if let (Some(parent_dir), Some(file_name)) = (&query.parent_dir, &query.file_name) {
        let file_path = if parent_dir == "/" {
            format!("/{file_name}")
        } else {
            format!("{}/{}", parent_dir.trim_end_matches('/'), file_name)
        };
        if let Some(bytes) = state
            .temp_file_manager
            .get_uploaded_bytes(&repo_id, &file_path)
            .await
            && bytes > uploaded_bytes as u64
        {
            uploaded_bytes = bytes as i64;
        }
    }

    let mut headers = HeaderMap::new();
    headers.insert("Accept-Ranges", "bytes".parse().unwrap());

    Ok((
        headers,
        Json(serde_json::json!({"uploadedBytes": uploaded_bytes})),
    ))
}

pub async fn get_block_download_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((repo_id, file_id, block_id)): Path<(String, String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.get("p").map(|s| s.as_str()).unwrap_or("/");
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let svc = FileService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
        state.token_manager.clone(),
        state.config.clone(),
        state.notification_manager.clone(),
    );

    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let url = svc
        .get_block_download_link(
            &repo_id,
            &file_id,
            &block_id,
            parent_dir,
            auth.user_id,
            &auth.email,
            host_header,
        )
        .await?;
    Ok(Json(url))
}

pub async fn delete_file_wrapper(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<crate::fs::handler::dir::V21DirQuery>,
) -> Result<Response, AppError> {
    delete_file_v21(auth, State(state), Path(repo_id), Query(query)).await
}
pub async fn delete_file_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<crate::fs::handler::dir::V21DirQuery>,
) -> Result<Response, AppError> {
    let json_result = super::dir::delete_dirent_v21(
        auth,
        axum::extract::State(state),
        axum::extract::Path((repo_id, "file".to_string())),
        axum::extract::Query(query),
    )
    .await?;
    Ok(json_result.into_response())
}
