use std::fmt::Write as _;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::repository::Repositories;
use crate::repository::repo_tag::TagInput;
use base::error::AppError;
use infra::common::util::{basename, get_head_root_id, parent_path_from, timestamp_rfc3339};
use infra::entity::repo_tag;
use infra::serialization::S_IFDIR;

/// Metadata-service-compatible helpers and operations.
///
/// The public API mirrors seafile's metadata service subset used by the mobile
/// clients (`GET/PUT metadata/`, `GET/PUT metadata/record/`, `GET metadata/tags/`,
/// `PUT metadata/file-tags/`). File records are addressed by a `record_id`
/// derived from the file path (hex-encoded), so no persistent record table is
/// required.
pub struct MetadataService {
    db: Arc<DatabaseConnection>,
    repos: Arc<Repositories>,
}

/// Resolved filesystem information for a single path.
struct FileInfo {
    size: i64,
    mtime: i64,
    modifier: String,
}

impl MetadataService {
    pub fn new(db: Arc<DatabaseConnection>, repos: Arc<Repositories>) -> Self {
        Self { db, repos }
    }

    // ── record_id helpers ─────────────────────────────────────────────────

    /// Derive an opaque, reversible `record_id` from a full file path.
    pub fn record_id_from_path(path: &str) -> String {
        let mut s = String::with_capacity(path.len() * 2);
        for b in path.as_bytes() {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Recover the file path from a `record_id`.
    pub fn path_from_record_id(record_id: &str) -> Result<String, AppError> {
        if record_id.is_empty() || !record_id.len().is_multiple_of(2) {
            return Err(AppError::BadRequest("invalid record id".into()));
        }
        let mut bytes = Vec::with_capacity(record_id.len() / 2);
        let raw = record_id.as_bytes();
        for i in (0..raw.len()).step_by(2) {
            let hi = hex_val(raw[i]);
            let lo = hex_val(raw[i + 1]);
            let (Some(hi), Some(lo)) = (hi, lo) else {
                return Err(AppError::BadRequest("invalid record id".into()));
            };
            bytes.push((hi << 4) | lo);
        }
        let path = String::from_utf8(bytes)
            .map_err(|_| AppError::BadRequest("invalid record id".into()))?;
        if !path.starts_with('/') {
            return Err(AppError::BadRequest("invalid record id".into()));
        }
        Ok(path)
    }

    // ── metadata config ───────────────────────────────────────────────────

    /// GET /metadata/ — config including the `tags_enabled` flag.
    pub async fn get_metadata_config(&self, repo_id: &str) -> Result<serde_json::Value, AppError> {
        let config = self.repos.metadata_config.find_by_repo_id(repo_id).await?;
        let enabled = config.as_ref().and_then(|c| c.enabled).unwrap_or(true);
        let tags_enabled = config.as_ref().and_then(|c| c.tags_enabled).unwrap_or(true);
        Ok(serde_json::json!({
            "enabled": enabled,
            "tags_enabled": tags_enabled,
            "details_settings": "{}",
            "global_hidden_columns": [],
            "face_recognition_enabled": false,
            "summary_enabled": false,
            "show_view": true,
        }))
    }

    /// PUT /metadata/ — enable/disable the metadata feature.
    pub async fn update_metadata_config(
        &self,
        repo_id: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        self.repos.metadata_config.upsert(repo_id, enabled).await
    }

    /// PUT/DELETE /metadata/tags-status/ — enable/disable the tag feature.
    pub async fn update_tags_enabled(&self, repo_id: &str, enabled: bool) -> Result<(), AppError> {
        self.repos
            .metadata_config
            .set_tags_enabled(repo_id, enabled)
            .await
    }

    // ── file records ──────────────────────────────────────────────────────

    /// GET /metadata/record/ — build a metadata-service-style record for a file.
    pub async fn get_file_record(
        &self,
        repo_id: &str,
        parent_dir: &str,
        file_name: &str,
    ) -> Result<serde_json::Value, AppError> {
        let parent_dir = if parent_dir.is_empty() {
            "/"
        } else {
            parent_dir
        };
        let file_path = if parent_dir == "/" {
            format!("/{file_name}")
        } else {
            format!("{parent_dir}/{file_name}")
        };

        let tags = self.tags_for_path(repo_id, &file_path).await?;
        let info = self.resolve_file_info(repo_id, &file_path).await?;

        let mut record = serde_json::json!({
            "_id": Self::record_id_from_path(&file_path),
            "_name": file_name,
            "_parent_dir": parent_dir,
            "_size": info.as_ref().map(|i| i.size).unwrap_or(0),
            "_file_mtime": info.as_ref().map(|i| timestamp_rfc3339(i.mtime)).unwrap_or_default(),
            "_file_modifier": info.as_ref().map(|i| i.modifier.clone()).unwrap_or_default(),
            "_tags": tags,
        });

        // Merge stored metadata fields (description/status/custom columns).
        let stored = self
            .repos
            .metadata_record
            .find_by_repo_and_path(repo_id, &file_path)
            .await?;
        if let serde_json::Value::Object(map) = &mut record {
            for r in stored {
                if let Some(v) = r.record_value {
                    map.insert(r.record_key, serde_json::Value::String(v));
                }
            }
        }

        Ok(serde_json::json!({
            "results": [record],
            "metadata": [],
        }))
    }

    /// PUT /metadata/record/ — store non-tag fields for a record.
    pub async fn update_file_record(
        &self,
        repo_id: &str,
        record_id: &str,
        data: &serde_json::Value,
    ) -> Result<(), AppError> {
        let file_path = Self::path_from_record_id(record_id)?;
        let mut fields: Vec<(String, Option<String>)> = Vec::new();
        if let Some(obj) = data.as_object() {
            for (k, v) in obj {
                if k == "_tags" || k == "_id" {
                    continue;
                }
                let value = match v {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(s) => Some(s.clone()),
                    other => Some(other.to_string()),
                };
                fields.push((k.clone(), value));
            }
        }
        self.repos
            .metadata_record
            .upsert_many(repo_id, &file_path, &fields)
            .await
    }

    // ── tags ──────────────────────────────────────────────────────────────

    /// GET /metadata/tags/ — list repo tags (metadata-service `results` shape).
    pub async fn list_repo_tags(
        &self,
        repo_id: &str,
        start: usize,
        limit: usize,
    ) -> Result<serde_json::Value, AppError> {
        let all = self.repos.repo_tag.find_by_repo_id(repo_id).await?;
        let slice = all.iter().skip(start).take(limit);
        let results: Vec<serde_json::Value> = slice.map(tag_to_json).collect();
        Ok(serde_json::json!({ "results": results, "metadata": [] }))
    }

    /// POST /metadata/tags/ — create tags (name-deduplicated).
    pub async fn create_repo_tags(
        &self,
        repo_id: &str,
        tags_data: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let mut inputs: Vec<TagInput> = Vec::new();
        if let Some(arr) = tags_data.as_array() {
            for item in arr {
                let name = item.get("_tag_name").and_then(|v| v.as_str());
                if let Some(name) = name {
                    if name.trim().is_empty() {
                        continue;
                    }
                    let color = item
                        .get("_tag_color")
                        .and_then(|v| v.as_str())
                        .unwrap_or("#e6e6e6")
                        .to_string();
                    inputs.push(TagInput {
                        name: name.to_string(),
                        color,
                    });
                }
            }
        }
        let tags = self.repos.repo_tag.create_many(repo_id, &inputs).await?;
        Ok(serde_json::json!({ "tags": tags.iter().map(tag_to_json).collect::<Vec<_>>() }))
    }

    /// PUT /metadata/tags/ — rename/recolor tags.
    pub async fn update_repo_tags(
        &self,
        repo_id: &str,
        tags_data: &serde_json::Value,
    ) -> Result<(), AppError> {
        let Some(arr) = tags_data.as_array() else {
            return Ok(());
        };
        for item in arr {
            let Some(tag_id) = item.get("tag_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(id) = tag_id.parse::<i32>() else {
                continue;
            };
            let Some(tag) = item.get("tag") else {
                continue;
            };
            let name = tag
                .get("_tag_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let color = tag
                .get("_tag_color")
                .and_then(|v| v.as_str())
                .unwrap_or("#e6e6e6");
            let existing = self.repos.repo_tag.find_by_id(id).await?;
            let Some(existing) = existing else {
                continue;
            };
            if existing.repo_id != repo_id {
                continue;
            }
            // Reject renaming to a name already used by another tag in this repo.
            if let Some(dup) = self
                .repos
                .repo_tag
                .find_by_repo_and_name(repo_id, name)
                .await?
                && dup.id != id
            {
                return Err(AppError::BadRequest("repo tag already exist".into()));
            }
            self.repos.repo_tag.update(id, name, color).await?;
        }
        Ok(())
    }

    /// DELETE /metadata/tags/ — delete tags by id.
    pub async fn delete_repo_tags(
        &self,
        repo_id: &str,
        tag_ids: &serde_json::Value,
    ) -> Result<(), AppError> {
        let Some(arr) = tag_ids.as_array() else {
            return Ok(());
        };
        for item in arr {
            let Some(s) = item.as_str() else {
                continue;
            };
            let Ok(id) = s.parse::<i32>() else {
                continue;
            };
            let Some(tag) = self.repos.repo_tag.find_by_id(id).await? else {
                continue;
            };
            if tag.repo_id == repo_id {
                self.repos.repo_tag.delete_by_id(id).await?;
            }
        }
        Ok(())
    }

    /// PUT /metadata/file-tags/ — set a file's tags (empty array clears them).
    pub async fn set_file_tags(
        &self,
        repo_id: &str,
        file_tags_data: &serde_json::Value,
    ) -> Result<(), AppError> {
        let Some(arr) = file_tags_data.as_array() else {
            return Ok(());
        };

        // Validate tags upfront so a bad id fails before any partial write.
        let repo_tags = self.repos.repo_tag.find_by_repo_id(repo_id).await?;
        let valid_ids: std::collections::HashSet<i32> = repo_tags.iter().map(|t| t.id).collect();

        for item in arr {
            let Some(record_id) = item.get("record_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let file_path = Self::path_from_record_id(record_id)?;

            let mut tag_ids: Vec<i32> = Vec::new();
            if let Some(tags) = item.get("tags").and_then(|v| v.as_array()) {
                for t in tags {
                    let Ok(id) = t.as_str().unwrap_or("").parse::<i32>() else {
                        return Err(AppError::BadRequest("invalid tag id".into()));
                    };
                    if !valid_ids.contains(&id) {
                        return Err(AppError::BadRequest("invalid tag id".into()));
                    }
                    tag_ids.push(id);
                }
            }
            self.repos
                .file_tag
                .set_for_path(repo_id, &file_path, &tag_ids)
                .await?;
        }
        Ok(())
    }

    /// GET /metadata/tag-files/{tag_id}/ — files carrying a tag.
    pub async fn get_tag_files(
        &self,
        repo_id: &str,
        tag_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        if let Some(tag) = self.repos.repo_tag.find_by_id(tag_id).await? {
            if tag.repo_id != repo_id {
                return Err(AppError::NotFound("tag not found".into()));
            }
        } else {
            return Err(AppError::NotFound("tag not found".into()));
        }

        let links = self
            .repos
            .file_tag
            .find_by_repo_and_tag_id(repo_id, tag_id)
            .await?;

        // Resolve the head root once, then batch-resolve every file's info and
        // tags instead of issuing several queries per link.
        let head_root_id = get_head_root_id(self.db.as_ref(), repo_id).await?;
        let paths: Vec<String> = links.iter().map(|l| l.file_path.clone()).collect();
        let infos = self
            .resolve_file_infos_batch(repo_id, &head_root_id, &paths)
            .await?;

        let tag_details = self
            .repos
            .file_tag
            .find_tag_details_by_paths(repo_id, &paths)
            .await?;
        let mut tags_by_path: std::collections::HashMap<String, Vec<serde_json::Value>> =
            std::collections::HashMap::new();
        for td in tag_details {
            tags_by_path
                .entry(td.file_path)
                .or_default()
                .push(serde_json::json!({
                    "row_id": td.tag_id.to_string(),
                    "display_value": td.tag_name,
                }));
        }

        let mut results = Vec::with_capacity(links.len());
        for (link, info) in links.iter().zip(infos.iter()) {
            let path = &link.file_path;
            let (parent_dir, name) = split_path(path);
            let file_tags = tags_by_path.get(path).cloned().unwrap_or_default();
            results.push(serde_json::json!({
                "_id": Self::record_id_from_path(path),
                "_name": name,
                "_parent_dir": parent_dir,
                "_size": info.as_ref().map(|i| i.size).unwrap_or(0),
                "_file_mtime": info.as_ref().map(|i| timestamp_rfc3339(i.mtime)).unwrap_or_default(),
                "_file_modifier": info.as_ref().map(|i| i.modifier.clone()).unwrap_or_default(),
                "_tags": file_tags,
            }));
        }
        Ok(serde_json::json!({ "results": results, "metadata": [] }))
    }

    // ── helpers ───────────────────────────────────────────────────────────

    async fn tags_for_path(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let links = self
            .repos
            .file_tag
            .find_by_repo_and_path(repo_id, file_path)
            .await?;
        // Batch-load the referenced tags in one query instead of one per link.
        let tag_ids: Vec<i32> = links.iter().map(|l| l.repo_tag_id).collect();
        let tags: std::collections::HashMap<i32, repo_tag::Model> = self
            .repos
            .repo_tag
            .find_by_ids(&tag_ids)
            .await?
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let mut out = Vec::with_capacity(links.len());
        for link in links {
            if let Some(tag) = tags.get(&link.repo_tag_id) {
                out.push(serde_json::json!({
                    "row_id": tag.id.to_string(),
                    "display_value": tag.name,
                }));
            }
        }
        Ok(out)
    }

    /// Best-effort resolution of a file's size/mtime/modifier from the FS tree.
    async fn resolve_file_info(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Option<FileInfo>, AppError> {
        if path == "/" || path.is_empty() {
            return Ok(None);
        }
        let db = self.db.as_ref();
        let head_root_id = get_head_root_id(db, repo_id).await?;
        let parent_path = parent_path_from(path);
        let file_name = basename(path);

        let parent_fs_id = match crate::fs::core::resolve_fs_id(
            self.repos.as_ref(),
            repo_id,
            &head_root_id,
            parent_path,
        )
        .await
        {
            Ok(id) => id,
            Err(_) => return Ok(None),
        };
        let parent_data =
            match crate::fs::core::read_fs_dir_data(self.repos.as_ref(), repo_id, &parent_fs_id)
                .await
            {
                Ok(d) => d,
                Err(_) => return Ok(None),
            };
        let Some(entry) = parent_data.dirents.iter().find(|e| e.name == file_name) else {
            return Ok(None);
        };
        if entry.mode & S_IFDIR != 0 {
            return Ok(None);
        }
        Ok(Some(FileInfo {
            size: entry.size,
            mtime: entry.mtime,
            modifier: entry.modifier.clone(),
        }))
    }

    /// Batch version of `resolve_file_info`: resolve `(size, mtime, modifier)`
    /// for many paths in a shared level-frontier walk. `None` for paths that
    /// are `/`, empty, directories, or no longer resolve.
    async fn resolve_file_infos_batch(
        &self,
        repo_id: &str,
        head_root_id: &str,
        paths: &[String],
    ) -> Result<Vec<Option<FileInfo>>, AppError> {
        let mut results: Vec<Option<FileInfo>> = (0..paths.len()).map(|_| None).collect();

        let mut targets: Vec<(String, String)> = Vec::new();
        let mut target_idx: Vec<usize> = Vec::new();
        let mut file_names: Vec<String> = Vec::new();

        for (i, path) in paths.iter().enumerate() {
            if path == "/" || path.is_empty() {
                continue;
            }
            targets.push((head_root_id.to_string(), parent_path_from(path).to_string()));
            target_idx.push(i);
            file_names.push(basename(path).to_string());
        }

        if targets.is_empty() {
            return Ok(results);
        }

        let resolved =
            crate::fs::core::resolve_fs_ids_batch(self.repos.as_ref(), repo_id, &targets).await?;

        let mut parent_ids: Vec<String> = resolved.iter().filter_map(|r| r.clone()).collect();
        parent_ids.sort();
        parent_ids.dedup();
        let dir_map =
            crate::fs::core::read_fs_dir_data_batch(self.repos.as_ref(), repo_id, &parent_ids)
                .await?;

        for (j, i) in target_idx.iter().enumerate() {
            let Some(parent_fs_id) = &resolved[j] else {
                continue;
            };
            let Some(dir_data) = dir_map.get(parent_fs_id) else {
                continue;
            };
            let Some(entry) = dir_data.dirents.iter().find(|e| e.name == file_names[j]) else {
                continue;
            };
            if entry.mode & S_IFDIR != 0 {
                continue; // directories are not files
            }
            results[*i] = Some(FileInfo {
                size: entry.size,
                mtime: entry.mtime,
                modifier: entry.modifier.clone(),
            });
        }

        Ok(results)
    }

    /// Related users (kept for mobile metadata profiles).
    pub async fn related_users(&self, repo_id: &str) -> Result<Vec<String>, AppError> {
        let members = self.repos.member.find_by_repo_id(repo_id).await?;
        Ok(members.into_iter().map(|m| m.user_id.to_string()).collect())
    }

    /// Get metadata records (existing minimal endpoint, kept as-is).
    pub async fn get_metadata_records(
        &self,
        repo_id: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let records = self.repos.metadata_record.find_by_repo_id(repo_id).await?;
        Ok(records
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "file_path": r.file_path,
                    "key": r.record_key,
                    "value": r.record_value,
                })
            })
            .collect())
    }

    /// Legacy key-value metadata record update.
    pub async fn update_metadata_record(
        &self,
        repo_id: &str,
        file_path: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), AppError> {
        self.repos
            .metadata_record
            .upsert(repo_id, file_path, key, value)
            .await
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

fn tag_to_json(tag: &repo_tag::Model) -> serde_json::Value {
    serde_json::json!({
        "_id": tag.id.to_string(),
        "_tag_name": tag.name,
        "_tag_color": tag.color,
        "_tag_file_links": [],
        "_creator": "",
        "_last_modifier": "",
        "_ctime": timestamp_rfc3339(tag.created_at),
        "_mtime": timestamp_rfc3339(tag.created_at),
    })
}

/// Split a full path into (parent_dir, file_name). Parent dir is `/` for root
/// files.
fn split_path(path: &str) -> (String, String) {
    let name = basename(path);
    let parent = parent_path_from(path);
    (
        if parent.is_empty() {
            "/".to_string()
        } else {
            parent.to_string()
        },
        name.to_string(),
    )
}
