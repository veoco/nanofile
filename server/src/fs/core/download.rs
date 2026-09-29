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

/// Decrypt a block on the blocking thread pool. AES-CBC is CPU-bound, so
/// running it inline on the async executor would stall other tasks sharing the
/// worker thread.
async fn decrypt_block_offload(
    data: Vec<u8>,
    key: &[u8],
    iv: &[u8],
) -> Result<Vec<u8>, std::io::Error> {
    let key = key.to_vec();
    let iv = iv.to_vec();
    // `decrypt_block` returns `Box<dyn Error>` (not `Send`), so map it to a
    // `String` inside the blocking closure to keep the join handle `Send`.
    tokio::task::spawn_blocking(move || decrypt_block(&data, &key, &iv).map_err(|e| e.to_string()))
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .map_err(std::io::Error::other)
}

impl Downloader {
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

        // Clamp from below before the cast: a negative `size` would otherwise
        // wrap to `usize::MAX` and abort the request with a capacity overflow.
        let mut out = Vec::with_capacity(file_data.size.max(0).min(max_bytes as i64) as usize);
        for block_id in &block_ids {
            if out.len() >= max_bytes {
                break;
            }
            let block_data = block_store
                .read_block(repo_id, block_id)
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            let block_data = if let Some((key, iv)) = dec_key {
                decrypt_block_offload(block_data, key, iv)
                    .await
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

    /// Read at most `max_bytes` of a file's content given its block IDs
    /// directly — the caller already resolved the file, so no tree walk or
    /// fs_object lookup is needed here. Returns fewer bytes when the file is
    /// smaller. Used by thumbnails after a single `resolve_file_entry`.
    pub async fn read_file_limited_from_blocks(
        repo_id: &str,
        block_store: &DynBlockStorage,
        block_ids: &[String],
        size: i64,
        dec_key: Option<(&[u8], &[u8])>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AppError> {
        // See `download_file_limited`: clamp before casting so a negative size
        // cannot wrap into a `usize::MAX` capacity hint.
        let mut out = Vec::with_capacity(size.max(0).min(max_bytes as i64) as usize);
        for block_id in block_ids {
            if out.len() >= max_bytes {
                break;
            }
            let block_data = block_store
                .read_block(repo_id, block_id)
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            let block_data = if let Some((key, iv)) = dec_key {
                decrypt_block_offload(block_data, key, iv)
                    .await
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

        Self::resolve_blocks_from_root(repos, repo_id, &head_commit.root_id, path).await
    }

    /// Resolve a file's block IDs from an already-resolved root fs_id, skipping
    /// the repo + head commit lookups. Callers that already resolved the head
    /// (e.g. WebDAV) reuse it here instead of re-querying.
    pub async fn resolve_blocks_from_root(
        repos: &Repositories,
        repo_id: &str,
        root_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        let fs_id = crate::fs::core::resolve_fs_id(repos, repo_id, root_id, path).await?;

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
    repo_id: String,
    block_ids: Vec<String>,
    block_store: DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    futures::stream::iter(block_ids.into_iter().map(move |block_id| {
        let store = block_store.clone();
        let repo_id = repo_id.clone();
        let key = enc_key.clone();
        async move {
            let data = store
                .read_block(&repo_id, &block_id)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let data = match &key {
                Some((k, iv)) => decrypt_block_offload(data, k, iv).await?,
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
    repo_id: String,
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
        let repo_id = repo_id.clone();
        let key = enc_key.clone();
        async move {
            if done {
                return None;
            }

            // Fast-forward past blocks that lie entirely before `start`, but
            // only when the reported sizes are known to be logical byte
            // offsets: the `Range` response is sliced by them, and nothing is
            // read while skipping, so a size that is even slightly off shifts
            // every byte after it (an at-rest decorator that mis-sized
            // header-less legacy blocks made clients' segmented downloads drop
            // bytes at the seam).
            //
            // - E2EE downloads report ciphertext lengths (padding included),
            //   which are not logical offsets, so `enc_key` disables the skip.
            // - A store that cannot guarantee `block_size == read_block().len()`
            //   must not be trusted to skip either.
            let mut expected_block_len: Option<u64> = None;
            if key.is_none() && store.logical_sizes_are_exact() && pos < start {
                loop {
                    // Peek the next block id without cloning the whole remaining
                    // iterator (cloning `IntoIter` copies every remaining entry,
                    // which is O(N²) for a large prefix skip).
                    let next_id = match iter.as_slice().first().cloned() {
                        Some(id) => id,
                        None => return Some((Ok(bytes::Bytes::new()), (iter, pos, true))),
                    };
                    let size = match store.block_size(&repo_id, &next_id).await {
                        Ok(s) if s >= 0 => s as u64,
                        _ => break, // fall through to the read path
                    };
                    if pos + size > start {
                        // The size that stopped the skip is also the offset
                        // accounting for this block; keep it so the read path can
                        // catch a store whose reported sizes drift.
                        expected_block_len = Some(size);
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
            let data = match store.read_block(&repo_id, &block_id).await {
                Ok(d) => d,
                Err(e) => {
                    return Some((Err(std::io::Error::other(e.to_string())), (iter, pos, done)));
                }
            };
            let data = match &key {
                Some((k, iv)) => match decrypt_block_offload(data, k, iv).await {
                    Ok(d) => d,
                    Err(e) => {
                        return Some((Err(e), (iter, pos, done)));
                    }
                },
                None => data,
            };
            let data = bytes::Bytes::from(data);

            // The prefix skip decided this block's start offset from its
            // reported size. If the bytes do not match that size, the offsets
            // are wrong and every byte of the response would be shifted: fail
            // the request instead of serving a silently mangled range.
            if let Some(expected) = expected_block_len
                && data.len() as u64 != expected
            {
                return Some((
                    Err(std::io::Error::other(format!(
                        "block {block_id} is {} bytes but its reported size was {expected}",
                        data.len()
                    ))),
                    (iter, pos, true),
                ));
            }

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

/// RFC 5987 percent-encode a filename for `filename*=utf-8''...`.
/// Keeps ASCII alphanumerics and `!#$&+-.^_`|~` unencoded, everything else
/// becomes `%XX` (UTF-8 bytes), matching the official fileserver.
fn rfc5987_encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// ASCII-only fallback for the `filename=` parameter when the real name has
/// non-ASCII characters. The raw UTF-8 bytes would make the header value
/// invalid (`HeaderValue::from_str` accepts only visible ASCII), which used to
/// drop the whole disposition to a bare `attachment` — losing the filename.
/// Keeps the ASCII part (usually the extension) so legacy clients still get
/// something useful; the full name travels via `filename*=utf-8''…`.
fn ascii_fallback(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_graphic() || c == ' ' {
            out.push(c);
        }
    }
    let trimmed = out.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        trimmed.replace('\\', "\\\\").replace('"', "\\\"")
    }
}

/// Build a `Content-Disposition` value in the official fileserver format
/// (see fileserver/fileop.go): both the RFC 5987 encoded filename (for
/// Safari, which cannot parse raw UTF-8) and the raw filename are sent.
/// `attachment` selects `attachment` vs `inline` disposition.
pub fn content_disposition(filename: &str, attachment: bool) -> String {
    let mode = if attachment { "attachment" } else { "inline" };
    // Quote / backslash in the raw filename must not break the header value.
    let fallback = if filename.is_ascii() {
        filename.replace('\\', "\\\\").replace('"', "\\\"")
    } else {
        ascii_fallback(filename)
    };
    format!(
        "{mode};filename*=utf-8''{};filename=\"{fallback}\"",
        rfc5987_encode(filename)
    )
}

/// Parameters for building a file-download HTTP response with Range support.
pub struct FileDownloadParams {
    /// Repository the blocks belong to: the per-repository block layout means
    /// every read must name the library it is allowed to read from.
    pub repo_id: String,
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
    /// Strong validator (e.g. `"<sha1-of-blocks>"`) sent as `ETag` so browsers
    /// can revalidate with `If-None-Match` instead of re-downloading the body.
    pub etag: Option<String>,
}

/// Build a streaming file response honoring a single `Range` request.
///
/// A satisfiable `Range` header yields `206 Partial Content` with
/// `Content-Range` and a slice stream; otherwise the full body is served with
/// `200 OK`. Both cases advertise `Accept-Ranges: bytes` and set
/// `Content-Length`, so download managers can display progress and resume an
/// interrupted transfer with a follow-up `Range` request.
/// Strong validator for a file body, derived from its content-addressed block
/// list: identical content always yields the same ETag and any edit changes it.
pub fn blocks_etag(block_ids: &[String]) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    for id in block_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"|");
    }
    format!("\"{}\"", hex::encode(hasher.finalize()))
}

pub fn file_download_response(p: FileDownloadParams) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(p.content_type),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    // Content is immutable for a given set of block IDs, so a long private
    // cache window plus a strong ETag lets browsers skip re-downloading large
    // originals across repeat views (matching the thumbnail cache policy).
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=604800"),
    );
    if let Some(etag) = p.etag.as_ref()
        && let Ok(v) = HeaderValue::from_str(etag)
    {
        headers.insert(header::ETAG, v);
    }
    if let Some(disposition) = p.content_disposition {
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition)
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
    }

    if let Some(range_header) = p.range_header.as_deref() {
        match parse_range(range_header, p.total_size) {
            Some((start, end)) => {
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
                let stream =
                    range_stream(p.repo_id, p.block_ids, p.block_store, p.enc_key, start, end);
                return (
                    StatusCode::PARTIAL_CONTENT,
                    headers,
                    Body::from_stream(stream),
                )
                    .into_response();
            }
            // A `Range` that does not describe a satisfiable single range is
            // 416 for a non-empty file, exactly as upstream's `doFileRange`
            // does — it never silently serves the whole file instead. A
            // zero-length file has no range to satisfy and stays 200.
            None if p.total_size > 0 => {
                headers.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{}", p.total_size))
                        .expect("Content-Range header value must be valid ASCII"),
                );
                return (StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response();
            }
            None => {}
        }
    }

    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&p.total_size.to_string())
            .expect("Content-Length header value must be valid ASCII"),
    );
    let stream = stream_blocks(p.repo_id, p.block_ids, p.block_store, p.enc_key);
    (StatusCode::OK, headers, Body::from_stream(stream)).into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        FileDownloadParams, content_disposition, file_download_response, parse_range, range_stream,
    };
    use axum::http::HeaderValue;
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
        async fn has_block(&self, _repo_id: &str, block_id: &str) -> bool {
            self.blocks.lock().unwrap().contains_key(block_id)
        }
        async fn read_block(
            &self,
            _repo_id: &str,
            block_id: &str,
        ) -> Result<Vec<u8>, std::io::Error> {
            self.blocks
                .lock()
                .unwrap()
                .get(block_id)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing block"))
        }
        async fn write_block(
            &self,
            _repo_id: &str,
            _data: &[u8],
        ) -> Result<String, std::io::Error> {
            unimplemented!()
        }
        async fn remove_block(
            &self,
            _repo_id: &str,
            _block_id: &str,
        ) -> Result<(), std::io::Error> {
            unimplemented!()
        }
        async fn block_size(&self, _repo_id: &str, block_id: &str) -> Result<i64, std::io::Error> {
            self.blocks
                .lock()
                .unwrap()
                .get(block_id)
                .map(|v| v.len() as i64)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing block"))
        }
        async fn list_blocks(&self, _repo_id: &str) -> Result<Vec<String>, std::io::Error> {
            unimplemented!()
        }
    }

    async fn collect_range(
        store: DynBlockStorage,
        block_ids: Vec<String>,
        start: u64,
        end: u64,
    ) -> Vec<u8> {
        range_stream("test-repo".to_string(), block_ids, store, None, start, end)
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

    #[test]
    fn content_disposition_ascii_name_unchanged() {
        assert_eq!(
            content_disposition("hello.txt", true),
            "attachment;filename*=utf-8''hello.txt;filename=\"hello.txt\""
        );
        // Quotes / backslashes stay escaped in both parts.
        assert_eq!(
            content_disposition("a\"b\\c.txt", false),
            "inline;filename*=utf-8''a%22b%5Cc.txt;filename=\"a\\\"b\\\\c.txt\""
        );
    }

    #[test]
    fn content_disposition_non_ascii_name_uses_ascii_fallback() {
        let d = content_disposition("中文报告.txt", true);
        // The whole value must be valid ASCII so HeaderValue::from_str accepts
        // it (previously the raw UTF-8 bytes dropped it to a bare `attachment`).
        assert!(
            HeaderValue::from_str(&d).is_ok(),
            "must be a valid header: {d}"
        );
        assert!(
            d.starts_with("attachment;filename*=utf-8''%E4%B8%AD%E6%96%87"),
            "full name must ride in filename*: {d}"
        );
        assert!(
            d.ends_with(r#"filename="txt""#),
            "fallback should keep the ASCII extension: {d}"
        );
    }

    #[test]
    fn content_disposition_all_non_ascii_falls_back_to_download() {
        let d = content_disposition("中文", true);
        assert!(
            HeaderValue::from_str(&d).is_ok(),
            "must be a valid header: {d}"
        );
        assert!(
            d.ends_with(r#"filename="download""#),
            "empty ASCII fallback should be 'download': {d}"
        );
    }

    // ── At-rest block formats vs. range offsets ─────────────────────────────

    /// Deterministic pseudo-random bytes; only the lengths matter here.
    fn pseudo_block(seed: u64, len: usize) -> Vec<u8> {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x9E37_79B9);
        (0..len)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) as u8
            })
            .collect()
    }

    /// Write `plaintexts` into an at-rest-encrypting store over a temp
    /// directory, then rewrite the first `legacy` blocks in the pre-`NFE1`
    /// header-less format — what a deployment that encrypted blocks before the
    /// versioned header existed left on disk.
    async fn at_rest_fixture(
        plaintexts: &[Vec<u8>],
        legacy: usize,
    ) -> (tempfile::TempDir, DynBlockStorage, Vec<String>) {
        use infra::crypto::block_encryption::{BlockCipher, HEADER_LEN};
        use infra::storage::block_store::BlockStorage;
        use infra::storage::encrypting_block_store::{BlockEncryptionMode, EncryptingBlockStore};

        let dir = tempfile::tempdir().unwrap();
        let raw = Arc::new(BlockStorage::new(dir.path().join("blocks")));
        let store: DynBlockStorage = Arc::new(EncryptingBlockStore::new(
            raw.clone(),
            BlockCipher::from_master_key(&[0x42u8; 32]),
            BlockEncryptionMode::On,
        ));

        let mut ids = Vec::new();
        for data in plaintexts {
            ids.push(store.write_block("test-repo", data).await.unwrap());
        }
        for id in ids.iter().take(legacy) {
            let stored = raw.read_block("test-repo", id).await.unwrap();
            assert_eq!(&stored[..4], b"NFE1", "fixture must start versioned");
            raw.write_block_with_id_force("test-repo", id, &stored[HEADER_LEN..])
                .await
                .unwrap();
            raw.invalidate_exists_cache();
        }
        (dir, store, ids)
    }

    /// A `Range` starting after header-less legacy blocks must deliver the
    /// exact tail. The incident: the at-rest store reported every legacy block
    /// 6 bytes short, so a client that segmented its download by those offsets
    /// (three skipped blocks) lost 18 bytes at the seam.
    #[tokio::test]
    async fn range_stream_keeps_offsets_with_headerless_at_rest_blocks() {
        let plaintexts: Vec<Vec<u8>> = [4096usize, 8192, 8192, 2048]
            .iter()
            .enumerate()
            .map(|(seed, len)| pseudo_block(seed as u64 + 1, *len))
            .collect();
        let (_dir, store, ids) = at_rest_fixture(&plaintexts, 3).await;
        let content: Vec<u8> = plaintexts.concat();

        // The offset a correct client derives from the block layout: the start
        // of the 4th block. The buggy sizes put it 18 bytes earlier, which is
        // where an offset-based client actually resumed from.
        let start = plaintexts[..3].iter().map(|d| d.len() as u64).sum::<u64>();
        let got = collect_range(store, ids, start, content.len() as u64 - 1).await;
        assert_eq!(got, content[start as usize..]);
    }

    /// The same slice, through the response builder the file-download handlers
    /// use, so status/headers and the stream are covered together.
    #[tokio::test]
    async fn file_range_response_serves_the_requested_slice_for_headerless_blocks() {
        let plaintexts: Vec<Vec<u8>> = [4096usize, 8192, 8192, 2048]
            .iter()
            .enumerate()
            .map(|(seed, len)| pseudo_block(seed as u64 + 100, *len))
            .collect();
        let (_dir, store, ids) = at_rest_fixture(&plaintexts, 3).await;
        let content: Vec<u8> = plaintexts.concat();
        let start = plaintexts[..3].iter().map(|d| d.len() as u64).sum::<u64>();

        let resp = file_download_response(FileDownloadParams {
            repo_id: "test-repo".to_string(),
            block_ids: ids,
            block_store: store,
            enc_key: None,
            total_size: content.len() as u64,
            content_type: "application/octet-stream",
            content_disposition: None,
            range_header: Some(format!("bytes={start}-")),
            etag: None,
        });
        assert_eq!(resp.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), &content[start as usize..]);
    }

    /// A store whose reported sizes are six bytes short must not be trusted to
    /// skip blocks: the skip turns those sizes into offsets and nothing is read
    /// while skipping.
    #[derive(Debug)]
    struct InexactSizeStore(MockStore);

    #[async_trait::async_trait]
    impl BlockStorageBackend for InexactSizeStore {
        async fn has_block(&self, repo_id: &str, block_id: &str) -> bool {
            self.0.has_block(repo_id, block_id).await
        }
        async fn read_block(
            &self,
            repo_id: &str,
            block_id: &str,
        ) -> Result<Vec<u8>, std::io::Error> {
            self.0.read_block(repo_id, block_id).await
        }
        async fn write_block(
            &self,
            _repo_id: &str,
            _data: &[u8],
        ) -> Result<String, std::io::Error> {
            unimplemented!()
        }
        async fn remove_block(
            &self,
            _repo_id: &str,
            _block_id: &str,
        ) -> Result<(), std::io::Error> {
            unimplemented!()
        }
        async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, std::io::Error> {
            Ok(self.0.block_size(repo_id, block_id).await? - 6)
        }
        async fn list_blocks(&self, _repo_id: &str) -> Result<Vec<String>, std::io::Error> {
            unimplemented!()
        }
        fn logical_sizes_are_exact(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn range_stream_does_not_skip_on_a_store_with_inexact_sizes() {
        let content: Vec<u8> = (0..160u8).collect();
        let mut blocks = HashMap::new();
        let mut ids = Vec::new();
        for (i, chunk) in content.chunks(40).enumerate() {
            let id = format!("blk{i}");
            blocks.insert(id.clone(), chunk.to_vec());
            ids.push(id);
        }
        let store: DynBlockStorage = Arc::new(InexactSizeStore(MockStore {
            blocks: Mutex::new(blocks),
        }));
        // Starting in the 4th block would skip three blocks; with sizes that
        // are six bytes short each, skipping would serve from 18 bytes ahead.
        let got = collect_range(store, ids, 120, 159).await;
        assert_eq!(got, content[120..]);
    }
}
