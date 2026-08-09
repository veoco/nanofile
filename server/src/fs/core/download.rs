use crate::repository::Repositories;
use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base::common::FsFileData;
use base::error::AppError;
use futures::{Stream, StreamExt};
use infra::crypto::random_key::decrypt_block;
use infra::storage::DynBlockStorage;

pub struct Downloader;

impl Downloader {
    pub async fn download_file(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
        block_store: &DynBlockStorage,
        // Optional decryption key (key, iv) — when set, blocks are decrypted
        // after reading. Used for encrypted repos during web download.
        dec_key: Option<(&[u8], &[u8])>,
    ) -> Result<Vec<u8>, AppError> {
        let (file_data, block_ids) = Self::download_file_stream(repos, repo_id, path).await?;

        let mut file_content = Vec::with_capacity(file_data.size as usize);
        for block_id in &block_ids {
            let block_data = block_store
                .read_block(block_id)
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            // If decryption key is provided, decrypt the block.
            let block_data = if let Some((key, iv)) = dec_key {
                decrypt_block(&block_data, key, iv)
                    .map_err(|e| AppError::internal(e.to_string()))?
            } else {
                block_data
            };
            file_content.extend_from_slice(&block_data);
        }

        Ok(file_content)
    }

    /// Read at most the first `max_bytes` of a file's content. Returns fewer
    /// bytes when the file is smaller. Used by previews and thumbnails so a
    /// huge file can't be loaded fully into memory.
    pub async fn download_file_limited(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
        block_store: &DynBlockStorage,
        dec_key: Option<(&[u8], &[u8])>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AppError> {
        let (file_data, block_ids) = Self::download_file_stream(repos, repo_id, path).await?;

        let mut out = Vec::with_capacity(file_data.size.min(max_bytes as i64) as usize);
        for block_id in &block_ids {
            if out.len() >= max_bytes {
                break;
            }
            let block_data = block_store
                .read_block(block_id)
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            let block_data = if let Some((key, iv)) = dec_key {
                decrypt_block(&block_data, key, iv)
                    .map_err(|e| AppError::internal(e.to_string()))?
            } else {
                block_data
            };
            let remaining = max_bytes - out.len();
            let take = remaining.min(block_data.len());
            out.extend_from_slice(&block_data[..take]);
            if take < block_data.len() {
                break;
            }
        }
        Ok(out)
    }

    /// Resolve a file's block IDs without reading their content.
    ///
    /// Returns `(FsFileData, Vec<block_id>)` so the caller can stream
    /// blocks individually without loading the entire file into memory.
    pub async fn resolve_blocks(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        Self::download_file_stream(repos, repo_id, path).await
    }

    pub async fn download_file_stream(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        // Resolve the path to a file fs_id by walking the FS tree from the
        // repo's head commit.
        let repo_model = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;
        let head_commit = repos
            .commit
            .find_by_repo_and_commit_id(repo_id, &head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        let fs_id =
            crate::fs::core::resolve_fs_id(repos, repo_id, &head_commit.root_id, path).await?;

        let file_data =
            crate::fs::core::file_ops::FileOps::read_file_fs_object(repos, repo_id, &fs_id).await?;

        Ok((file_data.clone(), file_data.block_ids))
    }
}

/// Build a streaming body that reads and yields blocks one at a time.
///
/// `block_ids` — list of block SHA-1 hashes to stream.
/// `block_store` — content-addressed block storage backend.
/// `enc_key` — optional decryption key (None = plaintext blocks).
pub fn stream_blocks(
    block_ids: Vec<String>,
    block_store: DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    futures::stream::iter(block_ids.into_iter().map(move |block_id| {
        let store = block_store.clone();
        let key = enc_key.clone();
        async move {
            let data = store
                .read_block(&block_id)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let data = match &key {
                Some((k, iv)) => {
                    decrypt_block(&data, k, iv).map_err(|e| std::io::Error::other(e.to_string()))?
                }
                None => data,
            };
            Ok(bytes::Bytes::from(data))
        }
    }))
    .buffered(4)
}

/// Parse a single-range `Range` request header against a known total size.
///
/// Handles `bytes=start-end`, `bytes=start-` and `bytes=-suffix`. Returns `None`
/// for malformed / multiple-range / unsatisfiable headers — the caller should
/// then serve the full body with status 200 instead of 206.
pub fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    // Only single ranges are supported; multiple ranges fall back to 200.
    if spec.contains(',') {
        return None;
    }
    if total == 0 {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    let start_s = start_s.trim();
    let end_s = end_s.trim();

    if start_s.is_empty() {
        // Suffix form: last N bytes.
        let n: u64 = end_s.parse().ok()?;
        if n == 0 {
            return None;
        }
        return Some((total.saturating_sub(n), total - 1));
    }

    let start: u64 = start_s.parse().ok()?;
    if start >= total {
        return None; // unsatisfiable — serve full body.
    }
    let end = if end_s.is_empty() {
        total - 1
    } else {
        end_s.parse::<u64>().ok()?.min(total - 1)
    };
    if start > end {
        return None;
    }
    Some((start, end))
}

/// Stream only the byte range `[start, end]` (inclusive) of a file that is
/// stored as a sequence of blocks.
///
/// Blocks are read sequentially and their byte offsets tracked cumulatively, so
/// no block-size metadata is required. Blocks entirely before `start` are read
/// and skipped; streaming stops once `end` is reached.
pub fn range_stream(
    block_ids: Vec<String>,
    block_store: DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
    start: u64,
    end: u64,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    let iter = block_ids.into_iter();
    // Track the cumulative byte offset starting from the file's first block.
    // (`start`/`end` are captured by the `move` closure below.)
    futures::stream::unfold((iter, 0u64, false), move |(mut iter, mut pos, done)| {
        let store = block_store.clone();
        let key = enc_key.clone();
        async move {
            if done {
                return None;
            }

            // Fast-forward past blocks that lie entirely before `start`. For
            // plaintext repos the stored block size equals the logical size, so
            // a cheap stat is enough — no need to read + decrypt a whole block
            // only to discard it (a tail `Range` request / resume would
            // otherwise read and decrypt nearly the entire file prefix).
            // Encrypted repos are skipped here because the ciphertext length
            // includes PKCS7 padding and cannot be used as a logical offset.
            if key.is_none() && pos < start {
                loop {
                    let next_id = match iter.clone().next() {
                        Some(id) => id,
                        None => return Some((Ok(bytes::Bytes::new()), (iter, pos, true))),
                    };
                    let size = match store.block_size(&next_id).await {
                        Ok(s) if s >= 0 => s as u64,
                        _ => break, // fall through to the read path
                    };
                    if pos + size > start {
                        break; // next block intersects the range
                    }
                    pos += size;
                    iter.next(); // consume the skipped block
                }
            }

            let block_id = match iter.next() {
                Some(id) => id,
                None => return Some((Ok(bytes::Bytes::new()), (iter, pos, true))),
            };
            let data = match store.read_block(&block_id).await {
                Ok(d) => d,
                Err(e) => {
                    return Some((Err(std::io::Error::other(e.to_string())), (iter, pos, done)));
                }
            };
            let data = bytes::Bytes::from(match &key {
                Some((k, iv)) => match decrypt_block(&data, k, iv) {
                    Ok(d) => d,
                    Err(e) => {
                        return Some((
                            Err(std::io::Error::other(e.to_string())),
                            (iter, pos, done),
                        ));
                    }
                },
                None => data,
            });

            let len = data.len() as u64;
            let block_start = pos;
            let block_end = block_start + len;
            pos = block_end;

            if block_start > end {
                return Some((Ok(bytes::Bytes::new()), (iter, pos, true)));
            }
            // Intersection of [block_start, block_end) with [start, end].
            let rel_start = start.saturating_sub(block_start);
            let rel_end = (end + 1).saturating_sub(block_start);
            if rel_start >= len {
                // Block lies entirely before the requested range — skip.
                return Some((Ok(bytes::Bytes::new()), (iter, pos, done)));
            }
            let take = rel_end.min(len) - rel_start.min(len);
            if take == 0 {
                return Some((Ok(bytes::Bytes::new()), (iter, pos, true)));
            }
            let chunk = data.slice(rel_start as usize..(rel_start + take) as usize);
            let finished = block_end > end;
            Some((Ok(chunk), (iter, pos, done || finished)))
        }
    })
}

/// Parameters for building a file-download HTTP response with Range support.
pub struct FileDownloadParams {
    pub block_ids: Vec<String>,
    pub block_store: DynBlockStorage,
    /// Optional decryption key (key, iv) — passed through to the streamers.
    pub enc_key: Option<(Vec<u8>, Vec<u8>)>,
    /// Total file size in bytes (used for `Content-Length` / `Content-Range`).
    pub total_size: u64,
    pub content_type: &'static str,
    /// `attachment; filename="..."` to force download, `None` for inline.
    pub content_disposition: Option<String>,
    /// Raw value of the request's `Range` header, if any.
    pub range_header: Option<String>,
}

/// Build a streaming file response honoring a single `Range` request.
///
/// A satisfiable `Range` header yields `206 Partial Content` with
/// `Content-Range` and a slice stream; otherwise the full body is served with
/// `200 OK`. Both cases advertise `Accept-Ranges: bytes` and set
/// `Content-Length`, so download managers can display progress and resume an
/// interrupted transfer with a follow-up `Range` request.
pub fn file_download_response(p: FileDownloadParams) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(p.content_type),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(disposition) = p.content_disposition {
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition)
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
    }

    if let Some((start, end)) = p.range_header.and_then(|r| parse_range(&r, p.total_size)) {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{}", p.total_size))
                .expect("Content-Range header value must be valid ASCII"),
        );
        headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&(end - start + 1).to_string())
                .expect("Content-Length header value must be valid ASCII"),
        );
        let stream = range_stream(p.block_ids, p.block_store, p.enc_key, start, end);
        return (
            StatusCode::PARTIAL_CONTENT,
            headers,
            Body::from_stream(stream),
        )
            .into_response();
    }

    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&p.total_size.to_string())
            .expect("Content-Length header value must be valid ASCII"),
    );
    let stream = stream_blocks(p.block_ids, p.block_store, p.enc_key);
    (StatusCode::OK, headers, Body::from_stream(stream)).into_response()
}

#[cfg(test)]
mod tests {
    use super::parse_range;
    use super::range_stream;
    use futures::StreamExt;
    use infra::storage::{BlockStorageBackend, DynBlockStorage};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// In-memory block store for exercising `range_stream` across block boundaries.
    #[derive(Debug)]
    struct MockStore {
        blocks: Mutex<HashMap<String, Vec<u8>>>,
    }

    #[async_trait::async_trait]
    impl BlockStorageBackend for MockStore {
        async fn has_block(&self, block_id: &str) -> bool {
            self.blocks.lock().unwrap().contains_key(block_id)
        }
        async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, std::io::Error> {
            self.blocks
                .lock()
                .unwrap()
                .get(block_id)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing block"))
        }
        async fn write_block(&self, _data: &[u8]) -> Result<String, std::io::Error> {
            unimplemented!()
        }
        async fn remove_block(&self, _block_id: &str) -> Result<(), std::io::Error> {
            unimplemented!()
        }
        async fn block_size(&self, block_id: &str) -> Result<i64, std::io::Error> {
            self.blocks
                .lock()
                .unwrap()
                .get(block_id)
                .map(|v| v.len() as i64)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing block"))
        }
        async fn list_blocks(&self) -> Result<Vec<String>, std::io::Error> {
            unimplemented!()
        }
    }

    async fn collect_range(
        store: DynBlockStorage,
        block_ids: Vec<String>,
        start: u64,
        end: u64,
    ) -> Vec<u8> {
        range_stream(block_ids, store, None, start, end)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flat_map(|r| r.unwrap().to_vec())
            .collect()
    }

    #[tokio::test]
    async fn range_stream_slices_single_block() {
        let content: Vec<u8> = (0..100u8).collect();
        let store = Arc::new(MockStore {
            blocks: Mutex::new(HashMap::from([("b1".to_string(), content.clone())])),
        });
        let got = collect_range(store, vec!["b1".to_string()], 10, 19).await;
        assert_eq!(got, &content[10..20]);
    }

    #[tokio::test]
    async fn range_stream_skips_earlier_blocks() {
        // Three blocks of 40 bytes each → file content is bytes 0..120.
        let mut blocks = HashMap::new();
        let mut content = Vec::new();
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            let chunk: Vec<u8> = ((i as u8) * 40..((i as u8) + 1) * 40).collect();
            blocks.insert(id.to_string(), chunk);
            content.extend_from_slice(&(i as u8 * 40..(i as u8 + 1) * 40).collect::<Vec<u8>>());
        }
        let store = Arc::new(MockStore {
            blocks: Mutex::new(blocks),
        });
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];

        // Range spans the middle block and part of the last block.
        let got = collect_range(store.clone(), ids.clone(), 50, 99).await;
        assert_eq!(got, &content[50..100]);

        // Open-ended range in the last block (end == total - 1, as parse_range yields).
        let got = collect_range(store.clone(), ids.clone(), 110, 119).await;
        assert_eq!(got, &content[110..120]);

        // Range starting at 0 → full content.
        let got = collect_range(store, ids, 0, 119).await;
        assert_eq!(got, content);
    }

    #[test]
    fn parse_range_full() {
        // No header → caller serves full body; parse_range on a nil header is None.
        assert_eq!(parse_range("", 1000), None);
    }

    #[test]
    fn parse_range_suffix() {
        assert_eq!(parse_range("bytes=-500", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-500", 300), Some((0, 299)));
        assert_eq!(parse_range("bytes=-0", 300), None);
    }

    #[test]
    fn parse_range_open_ended() {
        assert_eq!(parse_range("bytes=0-", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=999-", 1000), Some((999, 999)));
    }

    #[test]
    fn parse_range_bounded() {
        assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range("bytes=100-200", 1000), Some((100, 200)));
        // End beyond total is clamped.
        assert_eq!(parse_range("bytes=990-9999", 1000), Some((990, 999)));
    }

    #[test]
    fn parse_range_invalid() {
        // Start beyond total → unsatisfiable.
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=2000-3000", 1000), None);
        // Malformed.
        assert_eq!(parse_range("bytes=abc-", 1000), None);
        assert_eq!(parse_range("bytes=5", 1000), None);
        assert_eq!(parse_range("items=0-99", 1000), None);
        // Multiple ranges → unsupported.
        assert_eq!(parse_range("bytes=0-99,200-299", 1000), None);
        // Zero-length file.
        assert_eq!(parse_range("bytes=0-", 0), None);
    }
}
