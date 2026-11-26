//! File-download HTTP responses: Range, ETag, Content-Disposition and caching.
//!
//! The block/byte data plane stays in `fs::core::download`; this module owns only
//! the HTTP shape a handler or WebDAV response is built with.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use infra::storage::DynBlockStorage;

use crate::fs::core::download::{parse_range, range_stream, stream_blocks};
/// RFC 5987 percent-encode a filename for `filename*=utf-8''...`, matching the
/// official fileserver: ASCII alphanumerics and `!#$&+-.^_`|~` stay raw, else `%XX`.
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

/// ASCII-only fallback for the `filename=` parameter: `HeaderValue::from_str` accepts
/// only visible ASCII, so raw UTF-8 would drop the whole disposition to a bare
/// `attachment`. Keeps the ASCII part (usually the extension); the full name travels
/// via `filename*=`.
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

/// Build a `Content-Disposition` value like the official fileserver: both the RFC 5987
/// name (Safari cannot parse raw UTF-8) and the raw filename are sent.
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
/// How long a file response may be cached. Stable URLs (`/repos/...`,
/// share pages) must revalidate so an edit is visible immediately; capability
/// URLs are unique per request and can be cached briefly.
#[derive(Clone, Copy)]
pub enum CachePolicy {
    PrivateNoCache,
    PublicNoCache,
    MaxAge(u32),
}

impl CachePolicy {
    fn value(self) -> HeaderValue {
        match self {
            CachePolicy::PrivateNoCache => HeaderValue::from_static("private, no-cache"),
            CachePolicy::PublicNoCache => HeaderValue::from_static("public, no-cache"),
            CachePolicy::MaxAge(secs) => HeaderValue::from_str(&format!("max-age={secs}"))
                .expect("max-age header value must be valid ASCII"),
        }
    }
}

pub struct FileDownloadParams {
    /// Repository the blocks belong to: the per-repository block layout means
    /// every read must name the library it is allowed to read from.
    pub repo_id: String,
    pub block_ids: Vec<String>,
    pub block_store: DynBlockStorage,
    pub enc_key: Option<(Vec<u8>, Vec<u8>)>,
    pub total_size: u64,
    pub content_type: &'static str,
    pub content_disposition: Option<String>,
    pub range_header: Option<String>,
    /// Strong validator (e.g. `"<sha1-of-blocks>"`) sent as `ETag` so browsers
    /// can revalidate with `If-None-Match` instead of re-downloading the body.
    pub etag: Option<String>,
    /// Answer a `HEAD`: the status and every header a `GET` would carry, with no body and no
    /// block reads. WebDAV clients compare `HEAD` against `GET`, so the two must agree.
    pub head_only: bool,
    /// `Cache-Control` for the body. Stable URLs use a no-cache policy so an
    /// edited file is revalidated instead of served stale from the browser cache.
    pub cache: CachePolicy,
}

/// Strong validator for a file body: identical content always yields the same ETag.
pub fn blocks_etag(block_ids: &[String]) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    for id in block_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"|");
    }
    format!("\"{}\"", hex::encode(hasher.finalize()))
}

/// Whether a request's `If-None-Match` selects `etag` (`*` or a comma-separated list).
///
/// Comparison is the weak one RFC 9110 §13.1.2 specifies for this header, so a
/// `W/"…"` candidate matches the strong `"…"` validator.
pub fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    match headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        Some("*") => true,
        Some(value) => value.split(',').any(|candidate| {
            let candidate = candidate.trim();
            let candidate = candidate.strip_prefix("W/").unwrap_or(candidate);
            candidate == etag
        }),
        None => false,
    }
}

/// Build a streaming file response honoring a single `Range` request: a satisfiable
/// range yields `206` with `Content-Range`, otherwise the full body with `200`. Both
/// advertise `Accept-Ranges: bytes` and `Content-Length` so downloads can resume.
///
/// `If-None-Match` is evaluated first (RFC 9110 §13.1.2, and seafile-server's
/// `accessV2CB`): a matching validator answers `304` even when the request also
/// carries a `Range`, so the caller never builds a validator check of its own.
pub fn file_download_response(p: FileDownloadParams, request_headers: &HeaderMap) -> Response {
    if let Some(etag) = p.etag.as_deref()
        && if_none_match(request_headers, etag)
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        return response;
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(p.content_type),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    // The policy is per endpoint: a stable URL revalidates, a capability URL can
    // be cached for its short lifetime.
    headers.insert(header::CACHE_CONTROL, p.cache.value());
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
                // A HEAD answers with the same status and headers but reads no blocks.
                if p.head_only {
                    return (StatusCode::PARTIAL_CONTENT, headers).into_response();
                }
                let stream =
                    range_stream(p.repo_id, p.block_ids, p.block_store, p.enc_key, start, end);
                return (
                    StatusCode::PARTIAL_CONTENT,
                    headers,
                    Body::from_stream(stream),
                )
                    .into_response();
            }
            // An unsatisfiable single range is 416 for a non-empty file, as upstream's
            // `doFileRange` does; a zero-length file has no range and stays 200.
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
    if p.head_only {
        return (StatusCode::OK, headers).into_response();
    }
    let stream = stream_blocks(p.repo_id, p.block_ids, p.block_store, p.enc_key);
    (StatusCode::OK, headers, Body::from_stream(stream)).into_response()
}

#[cfg(test)]
mod tests {
    use super::{CachePolicy, FileDownloadParams, content_disposition, file_download_response};
    use crate::fs::core::download::{parse_range, range_stream};
    use axum::http::{HeaderMap, HeaderValue};
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

        let got = collect_range(store.clone(), ids.clone(), 50, 99).await;
        assert_eq!(got, &content[50..100]);

        let got = collect_range(store.clone(), ids.clone(), 110, 119).await;
        assert_eq!(got, &content[110..120]);

        let got = collect_range(store, ids, 0, 119).await;
        assert_eq!(got, content);
    }

    #[test]
    fn parse_range_full() {
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
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=2000-3000", 1000), None);
        assert_eq!(parse_range("bytes=abc-", 1000), None);
        assert_eq!(parse_range("bytes=5", 1000), None);
        assert_eq!(parse_range("items=0-99", 1000), None);
        assert_eq!(parse_range("bytes=0-99,200-299", 1000), None);
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
        // The whole value must be valid ASCII for HeaderValue::from_str.
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

    /// Write `plaintexts` into an at-rest-encrypting store, then rewrite the first
    /// `legacy` blocks in the pre-`NFE1` header-less format deployments left on disk.
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

    /// A `Range` starting after header-less legacy blocks must deliver the exact tail:
    /// the store reported every legacy block 6 bytes short, so a client segmenting by
    /// those offsets lost 18 bytes at the seam.
    #[tokio::test]
    async fn range_stream_keeps_offsets_with_headerless_at_rest_blocks() {
        let plaintexts: Vec<Vec<u8>> = [4096usize, 8192, 8192, 2048]
            .iter()
            .enumerate()
            .map(|(seed, len)| pseudo_block(seed as u64 + 1, *len))
            .collect();
        let (_dir, store, ids) = at_rest_fixture(&plaintexts, 3).await;
        let content: Vec<u8> = plaintexts.concat();

        // The offset a correct client derives from the block layout: the start of the
        // 4th block. The buggy sizes put it 18 bytes earlier — where clients resumed.
        let start = plaintexts[..3].iter().map(|d| d.len() as u64).sum::<u64>();
        let got = collect_range(store, ids, start, content.len() as u64 - 1).await;
        assert_eq!(got, content[start as usize..]);
    }

    /// The same slice through the response builder, so status/headers are covered too.
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

        let resp = file_download_response(
            FileDownloadParams {
                repo_id: "test-repo".to_string(),
                block_ids: ids,
                block_store: store,
                enc_key: None,
                total_size: content.len() as u64,
                content_type: "application/octet-stream",
                content_disposition: None,
                range_header: Some(format!("bytes={start}-")),
                etag: None,
                head_only: false,
                cache: CachePolicy::PrivateNoCache,
            },
            &HeaderMap::new(),
        );
        assert_eq!(resp.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), &content[start as usize..]);
    }

    /// A store whose reported sizes are six bytes short must not be trusted to
    /// skip: the skip turns those sizes into offsets and reads nothing meanwhile.
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
        // Sizes six bytes short each: skipping three blocks would serve 18 bytes ahead.
        let got = collect_range(store, ids, 120, 159).await;
        assert_eq!(got, content[120..]);
    }
}
