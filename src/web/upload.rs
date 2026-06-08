use axum::{
    Json,
    extract::{Multipart, Path, State},
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::error::AppError;
use crate::storage::file_ops::FileOps;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Compute the actual target directory given the base `parent_dir` and an
/// optional `relative_path` (e.g. `"myfolder/sub/"` from a folder upload).
/// When `relative_path` is empty, returns `parent_dir` unchanged.
fn compute_target_dir(parent_dir: &str, relative_path: &str) -> String {
    let rel = relative_path.trim_end_matches('/');
    if rel.is_empty() {
        return parent_dir.to_string();
    }
    if parent_dir == "/" {
        format!("/{}", rel)
    } else {
        format!("{}/{}", parent_dir.trim_end_matches('/'), rel)
    }
}

/// Create a file via `FileOps::create_file` and return the standard JSON
/// array response expected by the Seafile frontend:
/// `[{"id": "<fs_id>", "name": "<filename>", "size": <bytes>}]`
async fn upload_and_build_response(
    state: &AppState,
    repo_id: &str,
    target_dir: &str,
    filename: &str,
    data: &[u8],
    modifier: &str,
    replace: bool,
) -> Result<serde_json::Value, AppError> {
    let fs_id = FileOps::create_file(
        state.db.as_ref(),
        repo_id,
        target_dir,
        filename,
        data,
        modifier,
        replace,
        &state.block_store,
        Some(state.path_cache.as_ref()),
    )
    .await
    .map_err(|e| AppError::Internal(format!("upload failed: {e}")))?;

    Ok(json!([{"id": fs_id, "name": filename, "size": data.len()}]))
}

// ─── Multipart parser (for desktop-client compatibility) ──────────────────────

/// Simple multipart/form-data parser that finds a named field's value
/// and extracts the file (if any) from the multipart body.
///
/// Avoids axum's `Multipart` extractor which mysteriously fails with 400
/// on the desktop client's uploads (possibly a content-type encoding issue).
fn parse_multipart(data: &[u8], boundary: &str) -> MultipartResult {
    let boundary_str = format!("--{}", boundary);
    let btag = boundary_str.as_bytes();
    let crlf_btag = format!("\r\n--{}", boundary);
    let crlf_btag_bytes = crlf_btag.as_bytes();

    let mut result = MultipartResult {
        fields: std::collections::HashMap::new(),
        file_name: None,
        file_data: None,
    };

    let mut pos = 0;

    loop {
        // Find the next boundary (first one has no leading \r\n).
        let boundary_start = if pos == 0 && data[pos..].starts_with(btag) {
            pos
        } else if let Some(off) = data[pos..]
            .windows(crlf_btag_bytes.len())
            .position(|w| w == crlf_btag_bytes)
        {
            pos + off
        } else {
            break;
        };

        let mut boundary_end = boundary_start + btag.len();
        // Skip trailing \r\n or -- (closing)
        if boundary_end + 2 <= data.len() && &data[boundary_end..boundary_end + 2] == b"--" {
            break; // closing boundary
        }
        if boundary_end + 2 <= data.len() && &data[boundary_end..boundary_end + 2] == b"\r\n" {
            boundary_end += 2;
        }
        pos = boundary_end;

        // Find end of headers (\r\n\r\n)
        if let Some(hdr_end) = data[pos..].windows(4).position(|w| w == b"\r\n\r\n") {
            let hdr = String::from_utf8_lossy(&data[pos..pos + hdr_end]);
            pos += hdr_end + 4;

            // Body extends to the next boundary
            let body_end = if let Some(next_off) = data[pos..]
                .windows(crlf_btag_bytes.len())
                .position(|w| w == crlf_btag_bytes)
            {
                next_off
            } else {
                data.len() - pos
            };
            let body = &data[pos..pos + body_end];
            pos += body_end;

            // Trim trailing \r\n from body
            let body = body
                .strip_suffix(b"\r\n")
                .or_else(|| body.strip_suffix(b"\n"))
                .unwrap_or(body);

            let field_name = hdr
                .split(';')
                .find_map(|s| s.trim().strip_prefix("name=\"")?.strip_suffix('"'))
                .unwrap_or("");

            // Extract filename — handle both single-line and multi-line headers.
            // Qt client sends `filename="value"\r\nContent-Type: ...` so we
            // cannot use strip_suffix('"') (which would look for a trailing `"`
            // that isn't there). Instead, split on `"` to get the value.
            if let Some(fname) = hdr.split(';').find_map(|s| {
                let s = s.trim();
                s.strip_prefix("filename=\"")
                    .and_then(|rest| rest.split('"').next())
                    .map(|s| s.to_string())
            }) {
                result.file_name = Some(fname.to_string());
                result.file_data = Some(body.to_vec());
                result
                    .fields
                    .insert(field_name.to_string(), fname.to_string());
            } else {
                result.fields.insert(
                    field_name.to_string(),
                    String::from_utf8_lossy(body).to_string(),
                );
            }
        }
    }

    result
}

struct MultipartResult {
    fields: std::collections::HashMap<String, String>,
    file_name: Option<String>,
    file_data: Option<Vec<u8>>,
}

// ─── No-token web upload endpoints ────────────────────────────────────────────

/// POST /upload-aj/ — AJAX file upload (Seahub web UI, no token).
///
/// Expects multipart fields:
/// - `file` — the file bytes
/// - `repo_id` — repository ID
/// - `parent_dir` — target directory (default `/`)
/// - `relative_path` — subdirectory path for folder uploads (optional)
pub async fn upload_aj(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data = Vec::new();
    let mut filename = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            file_data = field.bytes().await.unwrap_or_default().to_vec();
        } else {
            fields.insert(name, field.text().await.unwrap_or_default());
        }
    }

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let parent_dir = fields.get("parent_dir").map(|s| s.as_str()).unwrap_or("/");
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let target_dir = compute_target_dir(parent_dir, relative_path);

    if !file_data.is_empty() {
        let resp = upload_and_build_response(
            &state,
            repo_id,
            &target_dir,
            &filename,
            &file_data,
            "web",
            false,
        )
        .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!([{"name": filename, "uploaded": true}])))
}

/// POST /update-api/ — Update existing file (web UI, no token).
///
/// Expects multipart fields:
/// - `file` — the new file bytes
/// - `repo_id` — repository ID
/// - `p` or `path` — full path of the target file (e.g. `/dir/file.txt`)
pub async fn update_api(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut repo_id = String::new();
    let mut file_path = String::new();
    let mut file_data: Vec<u8> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            file_data = field.bytes().await.unwrap_or_default().to_vec();
        } else {
            let val = field.text().await.unwrap_or_default();
            if name == "repo_id" {
                repo_id = val.clone();
            }
            if name == "p" || name == "path" {
                file_path = val;
            }
        }
    }

    if !file_data.is_empty() && !file_path.is_empty() {
        let parent = file_path
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .unwrap_or("/");
        let name = file_path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&file_path);

        let resp =
            upload_and_build_response(&state, &repo_id, parent, name, &file_data, "web", true)
                .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!({"success": true})))
}

/// POST /update-aj/ — AJAX file update (Seahub web UI).
///
/// Expects multipart fields:
/// - `file` — the new file bytes
/// - `repo_id` — repository ID
/// - `target_file` — full path (e.g. `/dir/file.txt`)
pub async fn update_aj(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data: Vec<u8> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            file_data = field.bytes().await.unwrap_or_default().to_vec();
        } else {
            fields.insert(name, field.text().await.unwrap_or_default());
        }
    }

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let target_file = fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("target_file required".into()))?;

    if !file_data.is_empty() {
        let parent = target_file
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .unwrap_or("/");
        let name = target_file
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(target_file);

        let resp =
            upload_and_build_response(&state, repo_id, parent, name, &file_data, "web", true)
                .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!({"success": true})))
}

// ─── Token-authenticated upload endpoints ─────────────────────────────────────

/// POST /upload-aj/{token} — Token-based AJAX file upload (Seahub web frontend).
///
/// This is the endpoint the Seahub React frontend sends uploads to after
/// obtaining an upload link from `/api2/repos/{id}/upload-link/?from=web`.
///
/// Multipart fields:
/// - `file` — the file bytes
/// - `parent_dir` — target directory
/// - `relative_path` — subdirectory path for folder uploads (optional)
pub async fn upload_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired upload token".into()))?;

    if info.op != "upload" {
        return Err(AppError::BadRequest("token not valid for upload".into()));
    }

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data: Vec<u8> = Vec::new();
    let mut filename = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            file_data = field.bytes().await.unwrap_or_default().to_vec();
        } else {
            fields.insert(name, field.text().await.unwrap_or_default());
        }
    }

    let parent_dir = fields
        .get("parent_dir")
        .map(|s| s.as_str())
        .unwrap_or(&info.parent_dir);
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let target_dir = compute_target_dir(parent_dir, relative_path);

    if !file_data.is_empty() {
        let resp = upload_and_build_response(
            &state,
            &info.repo_id,
            &target_dir,
            &filename,
            &file_data,
            &info.username,
            true, // replace behaviour (matching seafile-server default)
        )
        .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!([{"name": filename, "uploaded": true}])))
}

/// POST /upload-api/{token} — Token-authenticated file upload (desktop client).
///
/// Uses the custom `parse_multipart` helper instead of axum's `Multipart`
/// extractor because the Qt desktop client sends a quoted boundary string
/// that axum rejects with 400.
///
/// Multipart fields:
/// - `file` — the file bytes
/// - `parent_dir` — target directory (falls back to token's parent_dir)
/// - `relative_path` — subdirectory path for folder uploads (optional)
pub async fn upload_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Extract Content-Type header before consuming the request body.
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_default();

    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired upload token".into()))?;

    if info.op != "upload" {
        return Err(AppError::BadRequest("token not valid for upload".into()));
    }

    // Read the full body
    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Extract boundary from Content-Type
    // NOTE: Qt's QHttpMultiPart sends a quoted boundary
    // (`boundary="_.Seafile._UUID"`) while the body uses the unquoted form
    // (`--_.Seafile._UUID`). Strip surrounding quotes to handle both.
    let boundary = ct
        .split("boundary=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| AppError::BadRequest("missing boundary".into()))?;

    // Parse multipart using shared helper (avoids axum Multipart extractor
    // which mysteriously fails with 400 on the desktop client's uploads).
    let parsed = parse_multipart(&bytes, &boundary);
    let parent_dir = parsed
        .fields
        .get("parent_dir")
        .cloned()
        .unwrap_or(info.parent_dir);
    let relative_path = parsed
        .fields
        .get("relative_path")
        .cloned()
        .unwrap_or_default();
    let target_dir = compute_target_dir(&parent_dir, &relative_path);
    let filename = parsed.file_name.unwrap_or_default();

    if let Some(data) = parsed.file_data
        && !data.is_empty()
    {
        let resp = upload_and_build_response(
            &state,
            &info.repo_id,
            &target_dir,
            &filename,
            &data,
            &info.username,
            true, // replace existing files (seafile-server behavior)
        )
        .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!([{"name": filename, "uploaded": true}])))
}

// ─── Token-authenticated update endpoints ─────────────────────────────────────

/// POST /update-api/{token} — Token-authenticated file update / overwrite (desktop client).
///
/// Multipart fields (parsed via custom parser for Qt compat):
/// - `file` — the new file bytes
/// - `target_file` — full path of the file to overwrite (e.g. `/dir/file.txt`)
/// - `relative_path` — optional subdirectory path (prepended to target_file's parent)
/// - `parent_dir` — fallback base directory when target_file is absent
pub async fn update_api_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_default();

    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired update token".into()))?;

    if info.op != "update" {
        return Err(AppError::BadRequest("token not valid for update".into()));
    }

    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let parsed = ct
        .split("boundary=")
        .nth(1)
        .map(|s| parse_multipart(&bytes, s.trim().trim_matches('"')))
        .unwrap_or(MultipartResult {
            fields: std::collections::HashMap::new(),
            file_name: None,
            file_data: None,
        });

    let data = parsed.file_data.unwrap_or_default();
    let target_file = parsed
        .fields
        .get("target_file")
        .cloned()
        .unwrap_or_default();
    let relative_path = parsed
        .fields
        .get("relative_path")
        .cloned()
        .unwrap_or_default();

    if !data.is_empty() {
        if !target_file.is_empty() {
            // Derive target from target_file + optional relative_path
            let (raw_parent, raw_name) =
                target_file.rsplit_once('/').unwrap_or(("/", &target_file));
            let parent = if raw_parent.is_empty() {
                "/"
            } else {
                raw_parent
            };
            let target_dir = compute_target_dir(parent, &relative_path);
            let name = raw_name.to_string();

            let fs_id = FileOps::create_file(
                state.db.as_ref(),
                &info.repo_id,
                &target_dir,
                &name,
                &data,
                &info.username,
                true,
                &state.block_store,
                Some(state.path_cache.as_ref()),
            )
            .await
            .map_err(|e| AppError::Internal(format!("update failed: {e}")))?;

            return Ok(Json(
                json!([{"id": fs_id, "name": name, "size": data.len()}]),
            ));
        }

        // Fallback: parent_dir + relative_path + filename
        let filename = parsed.file_name.unwrap_or_default();
        let parent_dir = parsed
            .fields
            .get("parent_dir")
            .cloned()
            .unwrap_or(info.parent_dir);
        let target_dir = compute_target_dir(&parent_dir, &relative_path);

        if !filename.is_empty() {
            let resp = upload_and_build_response(
                &state,
                &info.repo_id,
                &target_dir,
                &filename,
                &data,
                &info.username,
                true,
            )
            .await?;
            return Ok(Json(resp));
        }
    }

    Ok(Json(json!({"success": true})))
}

/// POST /update-aj/{token} — Token-based AJAX file update (Seahub web frontend).
///
/// Multipart fields:
/// - `file` — the new file bytes
/// - `target_file` — full path (e.g. `/dir/file.txt`)
/// - `relative_path` — optional subdirectory path
pub async fn update_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired update token".into()))?;

    if info.op != "update" {
        return Err(AppError::BadRequest("token not valid for update".into()));
    }

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data: Vec<u8> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            file_data = field.bytes().await.unwrap_or_default().to_vec();
        } else {
            fields.insert(name, field.text().await.unwrap_or_default());
        }
    }

    if file_data.is_empty() {
        return Ok(Json(json!({"success": true})));
    }

    let target_file = fields.get("target_file").cloned().unwrap_or_default();
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();

    if !target_file.is_empty() {
        let slash_pos = target_file.rfind('/').unwrap_or(0);
        let raw_parent = if slash_pos == 0 {
            "/"
        } else {
            &target_file[..slash_pos]
        };
        let name = target_file[slash_pos + 1..].to_string();
        let target_dir = compute_target_dir(raw_parent, &relative_path);

        let fs_id = FileOps::create_file(
            state.db.as_ref(),
            &info.repo_id,
            &target_dir,
            &name,
            &file_data,
            &info.username,
            true,
            &state.block_store,
            Some(state.path_cache.as_ref()),
        )
        .await
        .map_err(|e| AppError::Internal(format!("update failed: {e}")))?;

        return Ok(Json(
            json!([{"id": fs_id, "name": name, "size": file_data.len()}]),
        ));
    }

    Ok(Json(json!({"success": true})))
}
