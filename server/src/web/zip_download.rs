//! ZIP download module — Seafile-compatible batch download as streaming zip.
//!
//! Implements:
//! - `POST /api/v2.1/repos/{repo_id}/zip-task/` — request a zip download token
//! - `GET /zip/{token}` — download the zip (streamed via `async_zip` + data descriptors)
//!
//! The ZIP stream uses **data descriptors** (`GeneralPurposeFlag.data_descriptor = true`)
//! so that each file entry's CRC-32 and sizes are written *after* the compressed data,
//! allowing true streaming without seeking back to patch the local file header.
//! See `async_zip::base::write::entry_stream::EntryStreamWriter::close()` for the
//! data-descriptor write (CRC-32 → compressed size → uncompressed size).

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::io::AsyncWriteExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::common::{EMPTY_SHA1, S_IFDIR};
use crate::error::AppError;
use crate::repo::fs_tree::{read_fs_dir_data, read_fs_file_data, resolve_fs_id};
use crate::repository::Repositories;

use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use tokio_util::io::ReaderStream;

// ── Data types ─────────────────────────────────────────────────────────

/// A file to be included in the zip archive.
#[allow(dead_code)]
struct ZipFileEntry {
    /// Path within the zip archive (e.g. `"myfolder/file.txt"`).
    path_in_zip: String,
    /// The content block IDs that make up this file's data.
    block_ids: Vec<String>,
    /// Uncompressed size in bytes.
    size: i64,
}

/// Task info stored per zip-token.
#[allow(dead_code)]
struct ZipTaskInfo {
    repo_id: String,
    files: Vec<ZipFileEntry>,
    // zip display name (without .zip extension)
    zip_name: String,
    created_at: i64,
}

// ── In-memory token store ──────────────────────────────────────────────

static ZIP_TASKS: OnceLock<Mutex<HashMap<String, ZipTaskInfo>>> = OnceLock::new();

fn zip_tasks() -> &'static Mutex<HashMap<String, ZipTaskInfo>> {
    ZIP_TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn generate_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// JSON payload for `POST zip-task/`.
#[derive(Deserialize)]
pub struct ZipTaskRequest {
    pub parent_dir: String,
    /// File/folder names within `parent_dir`.
    pub dirents: Vec<String>,
}

/// JSON response for `POST zip-task/`.
#[derive(serde::Serialize)]
pub struct ZipTaskResponse {
    pub zip_token: String,
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Look up the repo head commit's root fs_id.
async fn resolve_head_root(state: &AppState, repo_id: &str) -> Result<String, AppError> {
    let repo_model = state
        .repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = repo_model
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("Repository has no commits".into()))?;

    let head_commit = state
        .repos
        .commit
        .find_by_id(&head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    Ok(head_commit.root_id)
}

/// Recursively collect all files under `dir_path`.
///
/// `zip_prefix` is the path prefix that entries will have within the zip archive.
/// For a top-level directory `/myfolder`, `zip_prefix` would be `"myfolder"`.
async fn collect_files(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    dir_path: &str,
    zip_prefix: &str,
) -> Result<Vec<ZipFileEntry>, AppError> {
    let dir_id = if dir_path == "/" {
        root_fs_id.to_string()
    } else {
        resolve_fs_id(repos, repo_id, root_fs_id, dir_path)
            .await
            .map_err(|e| AppError::NotFound(format!("Path not found: {e}")))?
    };

    let mut entries = Vec::new();
    let mut stack: Vec<(String, String)> = vec![(dir_id, zip_prefix.to_string())];

    while let Some((fs_id, prefix)) = stack.pop() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }

        let dir_data = match read_fs_dir_data(repos, repo_id, &fs_id).await {
            Ok(d) => d,
            Err(_) => continue,
        };

        for dirent in &dir_data.dirents {
            let is_dir = dirent.mode & S_IFDIR != 0;
            let name = &dirent.name;

            let entry_path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };

            if is_dir {
                // Recurse into subdirectory
                stack.push((dirent.id.clone(), entry_path));
            } else {
                // Read file block IDs
                let file_data = match read_fs_file_data(repos, repo_id, &dirent.id).await {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                entries.push(ZipFileEntry {
                    path_in_zip: entry_path,
                    block_ids: file_data.block_ids,
                    size: file_data.size,
                });
            }
        }
    }

    Ok(entries)
}

/// Collect files for a set of selected dirents (names within parent_dir).
///
/// For each name, if it is a directory the whole subtree is included.
async fn collect_selected_files(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    parent_dir: &str,
    dirents: &[String],
) -> Result<Vec<ZipFileEntry>, AppError> {
    // Resolve parent_dir to get the listing of items within it
    let parent_dir_id = resolve_fs_id(repos, repo_id, root_fs_id, parent_dir)
        .await
        .map_err(|e| AppError::NotFound(format!("Parent dir not found: {e}")))?;

    let dir_data = read_fs_dir_data(repos, repo_id, &parent_dir_id)
        .await
        .map_err(|e| AppError::NotFound(format!("Not a directory: {e}")))?;

    let mut all_files = Vec::new();

    for name in dirents {
        // Find the entry in the parent directory
        let entry = dir_data
            .dirents
            .iter()
            .find(|d| d.name == *name)
            .ok_or_else(|| AppError::NotFound(format!("Entry not found: {name}")))?;

        let is_dir = entry.mode & S_IFDIR != 0;

        if is_dir {
            // Full subdirectory: walk from this dir
            let dir_path = if parent_dir == "/" {
                format!("/{name}")
            } else {
                format!("{parent_dir}/{name}")
            };
            let sub_files = collect_files(repos, repo_id, root_fs_id, &dir_path, name).await?;
            all_files.extend(sub_files);
        } else {
            // Single file
            let file_data = read_fs_file_data(repos, repo_id, &entry.id)
                .await
                .map_err(|_| AppError::NotFound(format!("File data not found: {name}")))?;

            all_files.push(ZipFileEntry {
                path_in_zip: name.clone(),
                block_ids: file_data.block_ids,
                size: file_data.size,
            });
        }
    }

    Ok(all_files)
}

/// Determine the zip filename (without extension) based on the request.
#[allow(unused_variables)]
fn determine_zip_name(parent_dir: &str, dirents: &[String]) -> String {
    if dirents.len() == 1 {
        // Single directory download → use directory name
        dirents[0].trim_end_matches('/').to_string()
    } else {
        // Multi-file download → use date-based name (matching seahub convention)
        let now = chrono::Local::now();
        format!("documents-export-{}", now.format("%Y-%m-%d"))
    }
}

// ── Stream zip via async_zip + duplex ─────────────────────────────────

/// Stream a zip archive over an HTTP response body.
///
/// Uses `tokio::io::duplex` to create a pipe: the zip writer writes into one
/// end and the HTTP response reads from the other end. `async_zip` writes
/// entries using **data descriptors** (streaming mode) so that no seeking
/// back is needed — the local file header has zero CRC/size, and the real
/// values are emitted after the compressed data.
fn stream_zip(
    block_store: crate::storage::DynBlockStorage,
    files: Vec<ZipFileEntry>,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    let (duplex_writer, duplex_reader) = tokio::io::duplex(64 * 1024);

    tokio::spawn(async move {
        let mut zip = ZipFileWriter::with_tokio(duplex_writer);

        for entry in &files {
            let builder =
                ZipEntryBuilder::new(entry.path_in_zip.clone().into(), Compression::Deflate);

            // Start a streaming entry — local file header has data_descriptor flag set,
            // CRC-32 and sizes are zeroed (will be written after data via data descriptor).
            let mut entry_writer = zip
                .write_entry_stream(builder)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;

            // Read each block and write it to the entry stream.
            // At any moment only one block (~2 MB) is held in memory.
            for block_id in &entry.block_ids {
                let data = block_store.read_block(block_id).await?;
                let data = if let Some((ref key, ref iv)) = enc_key {
                    crate::crypto::random_key::decrypt_block(&data, key, iv)
                        .map_err(|e| std::io::Error::other(e.to_string()))?
                } else {
                    data
                };
                entry_writer.write_all(&data).await?;
            }

            // Close the entry — this writes the data descriptor
            // (CRC-32, compressed size, uncompressed size) immediately after the data.
            entry_writer
                .close()
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }

        // Write central directory and end-of-central-directory record.
        zip.close()
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok::<(), std::io::Error>(())
    });

    ReaderStream::new(duplex_reader)
}

// ── Handlers ───────────────────────────────────────────────────────────

/// `POST /api/v2.1/repos/{repo_id}/zip-task/`
///
/// Accepts form data:
/// - `parent_dir` — the directory containing the items to download
/// - `dirents` — one or more file/folder names within `parent_dir`
///
/// Returns `{ "zip_token": "<uuid>" }` which the client can then pass to
/// `GET /zip/{token}` to receive the actual zip stream.
pub async fn zip_task_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(payload): Json<ZipTaskRequest>,
) -> Result<JsonResponse<ZipTaskResponse>, AppError> {
    let db = state.db.as_ref();

    // Verify read permission
    crate::storage::check_repo_read_permission(db, &repo_id, auth.user_id).await?;

    if payload.dirents.is_empty() {
        return Err(AppError::BadRequest(
            "No entries specified for download".into(),
        ));
    }

    // Resolve head commit root
    let root_fs_id = resolve_head_root(&state, &repo_id).await?;

    // Collect files (recursively for directories)
    let files = collect_selected_files(
        &state.repos,
        &repo_id,
        &root_fs_id,
        &payload.parent_dir,
        &payload.dirents,
    )
    .await?;

    if files.is_empty() {
        return Err(AppError::NotFound("No files to download".into()));
    }

    let zip_name = determine_zip_name(&payload.parent_dir, &payload.dirents);
    let token = generate_token();

    // Store task info
    let task = ZipTaskInfo {
        repo_id: repo_id.clone(),
        files,
        zip_name,
        created_at: now_secs(),
    };
    zip_tasks().lock().unwrap().insert(token.clone(), task);

    Ok(JsonResponse(ZipTaskResponse { zip_token: token }))
}

/// `GET /zip/{token}`
///
/// Streams the zip archive for a previously requested zip-task token.
/// The response has:
/// - `Content-Type: application/zip`
/// - `Content-Disposition: attachment; filename="<name>.zip"`
pub async fn zip_download_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    // Look up the task
    let task = {
        let mut tasks = zip_tasks().lock().unwrap();
        tasks
            .remove(&token)
            .ok_or_else(|| AppError::NotFound("Zip task not found or expired".into()))?
    };

    // Check if repo is encrypted and if password is set (for the user who created the task)
    // For simplicity with token-based access, we handle this case separately.
    // The token-based download doesn't carry user identity, so encrypted repos
    // without cached password will fail here.
    let dec_key: Option<(Vec<u8>, Vec<u8>)> = {
        let repo_model = state
            .repos
            .repo
            .find_by_id(&task.repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

        if repo_model.encrypted == 0 {
            None
        } else {
            // Encrypted repo: try to get from password manager
            // We don't have a user_id here, so we check all cached passwords.
            // This is a limitation — for encrypted repos, the two-step token flow
            // won't work. Users should use the direct download API instead.
            // For now, return an error for encrypted repos.
            return Err(AppError::BadRequest(
                "Zip download for encrypted repos is not supported via token. \
                 Use the direct download API instead."
                    .into(),
            ));
        }
    };

    let zip_filename = format!("{}.zip", task.zip_name);

    let stream = stream_zip(state.block_store.clone(), task.files, dec_key);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", zip_filename))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );

    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

// ── JsonResponse wrapper ───────────────────────────────────────────────

/// Wraps a serializable value into an `axum::Json` response.
pub struct JsonResponse<T: serde::Serialize>(pub T);

impl<T: serde::Serialize> IntoResponse for JsonResponse<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
