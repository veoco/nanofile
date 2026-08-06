//! Streaming ZIP archive helpers, shared by the zip-task handler
//! (`handler/web/zip_download.rs`) and the shared-link directory view
//! (`handler/web/share_view.rs`).

use futures::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};

use crate::fs::core::tree::{read_fs_dir_data, read_fs_file_data, resolve_fs_id};
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
            let entry_path = if prefix.is_empty() {
                dirent.name.clone()
            } else {
                format!("{prefix}/{}", dirent.name)
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

/// Stream a zip archive over an HTTP response body.
///
/// Uses `tokio::io::duplex` to create a pipe: the zip writer writes into one
/// end and the HTTP response reads from the other. `async_zip` writes entries
/// using **data descriptors** (streaming mode) so no seeking back is needed —
/// the local file header has zero CRC/size, and the real values are emitted
/// after the compressed data.
///
/// Each block is read one at a time (~2 MB) and decrypted when `enc_key` is
/// `Some`.
pub fn stream_zip(
    block_store: infra::storage::DynBlockStorage,
    files: Vec<ZipFileEntry>,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    let (duplex_writer, duplex_reader) = tokio::io::duplex(64 * 1024);

    tokio::spawn(async move {
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

            for block_id in &entry.block_ids {
                let data = block_store.read_block(block_id).await?;
                let data = if let Some((ref key, ref iv)) = enc_key {
                    infra::crypto::random_key::decrypt_block(&data, key, iv)
                        .map_err(|e| std::io::Error::other(e.to_string()))?
                } else {
                    data
                };
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
