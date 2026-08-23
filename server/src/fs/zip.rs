//! Streaming ZIP archive helpers, shared by the zip-task handler
//! (`handler/web/zip_download.rs`) and the shared-link directory view
//! (`handler/web/share_view.rs`).

use futures::StreamExt;
use futures::io::AsyncWriteExt;
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;

use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};

use crate::fs::core::tree::{
    read_fs_dir_data, read_fs_dir_data_batch, read_fs_file_data_batch, resolve_fs_id,
};
use crate::repository::Repositories;
use base::error::AppError;
use infra::common::{EMPTY_SHA1, S_IFDIR};

/// A file to be included in a zip archive.
pub struct ZipFileEntry {
    /// Path within the zip archive (e.g. `"myfolder/file.txt"`).
    pub path_in_zip: String,
    /// Content block IDs that make up this file's data.
    pub block_ids: Vec<String>,
    /// Uncompressed size in bytes.
    pub size: i64,
}

/// Cap on how many ZIP archives are generated concurrently across the server.
/// Each stream runs a deflate-heavy writer task plus block reads, so a flood of
/// large downloads could otherwise saturate CPU and disk. Callers queue on the
/// semaphore; the permit is released when the stream's writer task finishes.
const MAX_CONCURRENT_ZIPS: usize = 2;

static ZIP_CONCURRENCY: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn zip_semaphore() -> &'static Arc<Semaphore> {
    ZIP_CONCURRENCY.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_ZIPS)))
}

/// Recursively collect all files under `dir_path`.
///
/// `zip_prefix` is the path prefix entries will have inside the archive
/// (e.g. for a top-level directory `/myfolder`, `zip_prefix` would be
/// `"myfolder"`).
pub async fn collect_dir_entries(
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
    // Level frontier for directories; file ids are collected per level and
    // fetched in one batched query (O(#dirs)+O(#files) → O(depth)+1).
    let mut frontier: Vec<(String, String)> = vec![(dir_id, zip_prefix.to_string())];
    let mut pending_files: Vec<(String, String)> = Vec::new();

    while !frontier.is_empty() {
        let ids: Vec<String> = frontier
            .iter()
            .map(|(fs_id, _)| fs_id.clone())
            .filter(|id| id != EMPTY_SHA1)
            .collect();
        let dir_map = read_fs_dir_data_batch(repos, repo_id, &ids).await?;
        let mut next: Vec<(String, String)> = Vec::new();

        for (fs_id, prefix) in &frontier {
            // Missing/EMPTY dirs are absent from the batch map → skip.
            let Some(dir_data) = dir_map.get(fs_id) else {
                continue;
            };

            for dirent in &dir_data.dirents {
                let is_dir = dirent.mode & S_IFDIR != 0;
                let entry_path = if prefix.is_empty() {
                    dirent.name.clone()
                } else {
                    format!("{prefix}/{}", dirent.name)
                };

                if is_dir {
                    // Recurse into subdirectory
                    next.push((dirent.id.clone(), entry_path));
                } else {
                    pending_files.push((dirent.id.clone(), entry_path));
                }
            }
        }

        frontier = next;
    }

    // Fetch all file block IDs in one batched query.
    if !pending_files.is_empty() {
        let file_ids: Vec<String> = pending_files.iter().map(|(id, _)| id.clone()).collect();
        let file_map = read_fs_file_data_batch(repos, repo_id, &file_ids).await?;
        for (fs_id, entry_path) in pending_files {
            if let Some(file_data) = file_map.get(&fs_id) {
                entries.push(ZipFileEntry {
                    path_in_zip: entry_path,
                    block_ids: file_data.block_ids.clone(),
                    size: file_data.size,
                });
            }
        }
    }

    Ok(entries)
}

/// Collect files for a set of selected dirents (names within `parent_dir`).
///
/// For each name, if it is a directory the whole subtree is included.
pub async fn collect_selected_entries(
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
    let mut pending_files: Vec<(String, String)> = Vec::new();

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
            let sub_files =
                collect_dir_entries(repos, repo_id, root_fs_id, &dir_path, name).await?;
            all_files.extend(sub_files);
        } else {
            pending_files.push((entry.id.clone(), name.clone()));
        }
    }

    // Fetch all selected file block IDs in one batched query.
    if !pending_files.is_empty() {
        let file_ids: Vec<String> = pending_files.iter().map(|(id, _)| id.clone()).collect();
        let file_map = read_fs_file_data_batch(repos, repo_id, &file_ids).await?;
        for (fs_id, name) in pending_files {
            let file_data = file_map
                .get(&fs_id)
                .ok_or_else(|| AppError::NotFound(format!("File data not found: {name}")))?;
            all_files.push(ZipFileEntry {
                path_in_zip: name,
                block_ids: file_data.block_ids.clone(),
                size: file_data.size,
            });
        }
    }

    Ok(all_files)
}

/// Stream a zip archive over an HTTP response body.
///
/// Uses `tokio::io::duplex` to create a pipe: the zip writer writes into one
/// end and the HTTP response reads from the other. `async_zip` writes entries
/// using **data descriptors** (streaming mode) so no seeking back is needed —
/// the local file header has zero CRC/size, and the real values are emitted
/// after the compressed data.
///
/// Blocks are read + decrypted with bounded concurrency via
/// [`stream_blocks`](crate::fs::core::stream_blocks), then written into each
/// entry in order (the ZIP writer is single-threaded).
pub fn stream_zip(
    block_store: infra::storage::DynBlockStorage,
    files: Vec<ZipFileEntry>,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    let (duplex_writer, duplex_reader) = tokio::io::duplex(64 * 1024);

    tokio::spawn(async move {
        // Gate concurrent archive generation so a burst of large downloads
        // can't saturate the runtime. The permit drops when the writer task
        // finishes (including error paths), freeing a slot for the next zip.
        let _permit = zip_semaphore()
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| std::io::Error::other(format!("zip concurrency gate failed: {e}")))?;
        let mut zip = ZipFileWriter::with_tokio(duplex_writer);

        for entry in &files {
            let builder =
                ZipEntryBuilder::new(entry.path_in_zip.clone().into(), Compression::Deflate);

            // Start a streaming entry — local file header has the data_descriptor
            // flag set, CRC-32 and sizes are zeroed (written later via descriptor).
            let mut entry_writer = zip
                .write_entry_stream(builder)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;

            // Read + decrypt blocks with bounded concurrency, write in order.
            // stream_blocks buffers (default 4) so disk I/O and decryption
            // overlap across blocks while preserving block order.
            let mut blocks = Box::pin(crate::fs::core::stream_blocks(
                entry.block_ids.clone(),
                block_store.clone(),
                enc_key.clone(),
            ));
            while let Some(data) = blocks.next().await {
                let data = data?;
                entry_writer.write_all(&data).await?;
            }

            // Close the entry — this writes the data descriptor
            // (CRC-32, compressed size, uncompressed size) after the data.
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
