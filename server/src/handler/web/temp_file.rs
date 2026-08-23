// ─── TempFileManager ─────────────────────────────────────────────────────
//!
//! Manages temporary files for resumable/chunked uploads.
//!
//! Uses an in-memory `HashMap` to track active uploads; on server restart
//! all leftover temp files in `{temp_dir}/upload/` are wiped clean.
//!
//! Thread-safe (Arc<RwLock<...>>), designed for concurrent chunk writes.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures::stream::StreamExt;
use infra::storage::DynBlockStorage;
use infra::storage::cdc::Chunker;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

/// Manages per-upload temporary files on disk with an in-memory index.
#[derive(Clone)]
pub struct TempFileManager {
    inner: Arc<Inner>,
}

/// Active temp uploads plus the running total of their declared sizes. Both
/// live under one write lock so the reserved-bytes quota is enforced
/// atomically with the entry map.
struct ActiveUploads {
    /// (repo_id, file_path_in_repo) → active temp file entry
    entries: HashMap<(String, String), TempFileEntry>,
    /// Sum of declared `file_size` over active entries.
    reserved_bytes: u64,
}

struct Inner {
    /// All active temp uploads.
    active: RwLock<ActiveUploads>,
    /// Root directory for upload temp files, e.g. `data/temp`
    temp_dir: PathBuf,
    /// Cap on concurrent active uploads (0 = unlimited).
    max_uploads: u64,
    /// Cap on total reserved bytes across active uploads (0 = unlimited).
    max_bytes: u64,
}

struct TempFileEntry {
    tmp_path: PathBuf,
    /// Total file size as declared in the first Content-Range header
    file_size: u64,
    created_at: Instant,
    /// Per-upload CDC streaming state. An in-order resumable upload streams
    /// each chunk straight into blocks as it arrives (see `feed_stream`), so
    /// the final chunk commits without re-reading the whole temp file.
    stream: Arc<Mutex<Option<UploadStream>>>,
}

/// Incremental CDC state for one resumable upload.
///
/// `feed_stream` feeds in-order chunks through a [`Chunker`] and writes the
/// completed blocks to the block store, accumulating their ids. Any chunk that
/// arrives out of order (or with a write failure) flips `broken`, after which
/// the upload falls back to the plain temp-file assembly path.
struct UploadStream {
    /// Active chunker; created on the first in-order feed.
    chunker: Option<Chunker>,
    /// Bytes successfully fed into `chunker` so far (the in-order prefix).
    next_offset: u64,
    /// Block ids for the completed blocks produced so far, in order.
    block_ids: Vec<String>,
    /// Total declared file size, copied from the entry so the stream is
    /// self-contained once the map lock is released.
    file_size: u64,
    /// Set on the first out-of-order chunk or block-write failure.
    broken: bool,
}

/// Outcome of feeding one chunk to an upload's streaming CDC state.
pub enum FeedOutcome {
    /// The chunk was in order and its completed blocks were persisted.
    Streamed { block_ids: Vec<String> },
    /// The chunk was out of order or a write failed; streaming is disabled.
    Broken,
}

impl TempFileManager {
    /// Create a new manager and clean up any leftover temp files from a
    /// previous run by removing `{temp_dir}/upload/` entirely.
    ///
    /// `max_uploads` and `max_bytes` bound concurrent active uploads (count)
    /// and the sum of their declared sizes (bytes); `0` disables each limit.
    pub async fn new(temp_dir: PathBuf, max_uploads: u64, max_bytes: u64) -> Self {
        let upload_dir = temp_dir.join("upload");
        if upload_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&upload_dir).await {
                tracing::warn!(
                    "Failed to clean stale upload temp dir {:?}: {e}",
                    upload_dir
                );
            } else {
                tracing::debug!("Cleaned stale upload temp dir {:?}", upload_dir);
            }
        }
        Self {
            inner: Arc::new(Inner {
                active: RwLock::new(ActiveUploads {
                    entries: HashMap::new(),
                    reserved_bytes: 0,
                }),
                temp_dir,
                max_uploads,
                max_bytes,
            }),
        }
    }

    /// Return or create the temp file path for a given upload.
    /// On first call for a given (repo_id, file_path), creates a new unique
    /// temp file and records it in the in-memory index.
    ///
    /// Rejects the upload with `ErrorKind::QuotaExceeded` when the configured
    /// active-upload count or total reserved-bytes cap would be exceeded, so an
    /// attacker (e.g. an anonymous upload link) cannot pile up unbounded temp
    /// files on disk or entries in memory by starting abandoned uploads.
    pub async fn get_or_create(
        &self,
        repo_id: &str,
        file_path: &str,
        file_size: u64,
    ) -> std::io::Result<PathBuf> {
        let key = (repo_id.to_string(), file_path.to_string());
        let mut guard = self.inner.active.write().await;

        if let Some(entry) = guard.entries.get(&key) {
            return Ok(entry.tmp_path.clone());
        }

        if self.inner.max_uploads > 0 && guard.entries.len() as u64 >= self.inner.max_uploads {
            return Err(std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "too many concurrent uploads",
            ));
        }
        if self.inner.max_bytes > 0
            && guard.reserved_bytes.saturating_add(file_size) > self.inner.max_bytes
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "temp upload byte quota exceeded",
            ));
        }

        let dir = self.inner.temp_dir.join("upload").join(repo_id);
        fs::create_dir_all(&dir).await?;

        let tmp_path = dir.join(Uuid::new_v4().to_string());
        // Create an empty file so other chunks can open it for writing
        fs::write(&tmp_path, &[]).await?;

        guard.reserved_bytes += file_size;
        guard.entries.insert(
            key,
            TempFileEntry {
                tmp_path: tmp_path.clone(),
                file_size,
                created_at: Instant::now(),
                stream: Arc::new(Mutex::new(Some(UploadStream {
                    chunker: None,
                    next_offset: 0,
                    block_ids: Vec::new(),
                    file_size,
                    broken: false,
                }))),
            },
        );

        Ok(tmp_path)
    }

    /// Write `data` at `offset` into the temp file identified by
    /// (repo_id, file_path).  The file must already exist (via
    /// `get_or_create`).
    ///
    /// Rejects with `ErrorKind::InvalidInput` when the chunk would extend past
    /// the file's declared size — otherwise a client could write at an
    /// arbitrary offset and create a huge sparse file on disk.
    pub async fn write_chunk(
        &self,
        repo_id: &str,
        file_path: &str,
        offset: u64,
        data: &[u8],
    ) -> std::io::Result<()> {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let guard = self.inner.active.read().await;
            let entry = guard.entries.get(&key).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no active temp file for this upload",
                )
            })?;
            if offset.saturating_add(data.len() as u64) > entry.file_size {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "chunk extends beyond the declared file size",
                ));
            }
            entry.tmp_path.clone()
        };

        let data = data.to_vec(); // clone for the blocking closure
        tokio::task::spawn_blocking(move || {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(false)
                .open(&tmp_path)?;
            f.seek(SeekFrom::Start(offset))?;
            f.write_all(&data)?;
            Ok::<_, std::io::Error>(())
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))??;

        Ok(())
    }

    /// Stream an in-order chunk through the upload's CDC chunker and persist
    /// any completed blocks, so a fully in-order resumable upload can commit
    /// on the final chunk without re-reading the assembled temp file.
    ///
    /// The chunk is only consumed when it is strictly contiguous with the
    /// previously streamed prefix (`offset == next_offset`). Any other chunk —
    /// out of order, overlapping, or re-sent — disables streaming for the
    /// upload; correctness is preserved by the always-written temp file, which
    /// the caller falls back to on the final chunk.
    pub async fn feed_stream(
        &self,
        store: &DynBlockStorage,
        repo_id: &str,
        file_path: &str,
        offset: u64,
        data: &[u8],
    ) -> FeedOutcome {
        // Read the stream handle under the map lock, then release it before
        // the long-running stream lock + block writes (no lock nesting).
        let stream_handle = {
            let guard = self.inner.active.read().await;
            match guard
                .entries
                .get(&(repo_id.to_string(), file_path.to_string()))
            {
                Some(e) => e.stream.clone(),
                None => return FeedOutcome::Broken,
            }
        };
        let mut stream = stream_handle.lock().await;
        let Some(state) = stream.as_mut() else {
            return FeedOutcome::Broken;
        };
        if state.broken || offset != state.next_offset {
            state.broken = true;
            return FeedOutcome::Broken;
        }

        let chunker = state
            .chunker
            .get_or_insert_with(|| Chunker::new(state.file_size as usize));
        let mut ids = Vec::new();
        for blk in chunker.feed(data) {
            match store.write_block(&blk).await {
                Ok(id) => ids.push(id),
                Err(_) => {
                    // The chunker has advanced but the block is lost; disable
                    // streaming and rely on the temp file for assembly.
                    state.broken = true;
                    return FeedOutcome::Broken;
                }
            }
        }
        state.block_ids.extend(ids.iter().cloned());
        state.next_offset += data.len() as u64;
        FeedOutcome::Streamed { block_ids: ids }
    }

    /// Consume the fully-streamed block ids for an upload, if it streamed the
    /// whole file in order.
    ///
    /// Returns `Some((block_ids, total_size))` only when the stream is intact
    /// and covered the entire declared file (`next_offset == file_size ==
    /// expected_size`). `expected_size` is a defensive cross-check against a
    /// client changing `file_size` mid-upload. Otherwise returns `None` and the
    /// caller falls back to reading the temp file.
    pub async fn take_streamed_blocks(
        &self,
        store: &DynBlockStorage,
        repo_id: &str,
        file_path: &str,
        expected_size: u64,
    ) -> Option<(Vec<String>, i64)> {
        let stream_handle = {
            let guard = self.inner.active.read().await;
            guard
                .entries
                .get(&(repo_id.to_string(), file_path.to_string()))
                .map(|e| e.stream.clone())?
        };
        let mut stream = stream_handle.lock().await;
        let state = stream.as_mut()?;
        if state.broken || state.next_offset != state.file_size || state.file_size != expected_size
        {
            return None;
        }

        // Take the chunker and disable further feeds so a concurrent chunk
        // cannot interleave with the trailing `finish()`.
        let chunker = state.chunker.take()?;
        state.broken = true;
        let tail = chunker.finish();
        if !tail.is_empty() {
            match store.write_block(&tail).await {
                Ok(id) => state.block_ids.push(id),
                Err(_) => return None,
            }
        }
        Some((std::mem::take(&mut state.block_ids), state.file_size as i64))
    }

    /// How many bytes have been written to the temp file so far?
    /// Returns `None` when no temp file exists for this upload.
    pub async fn get_uploaded_bytes(&self, repo_id: &str, file_path: &str) -> Option<u64> {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let guard = self.inner.active.read().await;
            guard.entries.get(&key).map(|e| e.tmp_path.clone())?
        };
        match fs::metadata(&tmp_path).await {
            Ok(m) => Some(m.len()),
            Err(_) => None,
        }
    }

    /// Read the complete temp file into memory.
    /// The caller should call this only after the last chunk has been
    /// written (i.e. the file is fully assembled).
    pub async fn read_complete(&self, repo_id: &str, file_path: &str) -> Option<Vec<u8>> {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let guard = self.inner.active.read().await;
            guard.entries.get(&key).map(|e| e.tmp_path.clone())?
        };
        fs::read(&tmp_path).await.ok()
    }

    /// Stream the assembled temp file from disk in bounded chunks, so the
    /// final chunk-assembly path never has to read the whole file into memory.
    /// Returns `None` when no active temp file exists for this upload.
    pub async fn read_stream(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Option<futures::stream::BoxStream<'static, std::io::Result<bytes::Bytes>>> {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let guard = self.inner.active.read().await;
            guard.entries.get(&key).map(|e| e.tmp_path.clone())?
        };
        let file = tokio::fs::File::open(&tmp_path).await.ok()?;
        Some(
            futures::stream::try_unfold(file, |f| async move {
                let mut f = f;
                let mut buf = vec![0u8; 64 * 1024];
                let n = f.read(&mut buf).await?;
                if n == 0 {
                    return Ok(None);
                }
                buf.truncate(n);
                Ok(Some((bytes::Bytes::from(buf), f)))
            })
            .boxed(),
        )
    }

    /// Mark an upload as finished: remove the in-memory record and delete
    /// the temporary file from disk.
    pub async fn finish(&self, repo_id: &str, file_path: &str) {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let mut guard = self.inner.active.write().await;
            let removed = guard.entries.remove(&key);
            if let Some(e) = &removed {
                guard.reserved_bytes -= e.file_size;
            }
            removed.map(|e| e.tmp_path)
        };
        if let Some(p) = tmp_path {
            let _ = fs::remove_file(&p).await;
        }
    }

    /// Abort an upload: same as `finish` but also logs a warning.
    pub async fn abort(&self, repo_id: &str, file_path: &str) {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let mut guard = self.inner.active.write().await;
            let removed = guard.entries.remove(&key);
            if let Some(e) = &removed {
                guard.reserved_bytes -= e.file_size;
            }
            removed.map(|e| e.tmp_path)
        };
        if let Some(p) = tmp_path {
            let _ = fs::remove_file(&p).await;
        }
    }

    /// Remove active uploads that have been idle longer than `ttl` and delete
    /// their temp files from disk. Called periodically by the scheduler so
    /// abandoned resumable uploads don't leak memory or disk.
    pub async fn cleanup_stale(&self, ttl: std::time::Duration) {
        let cutoff = Instant::now() - ttl;
        let stale: Vec<PathBuf> = {
            let mut guard = self.inner.active.write().await;
            let mut paths = Vec::new();
            // Collect the stale keys first so the closure doesn't borrow both
            // `entries` and `reserved_bytes` from `guard` at once.
            let stale_keys: Vec<(String, String)> = guard
                .entries
                .iter()
                .filter(|(_, e)| e.created_at < cutoff)
                .map(|(k, _)| k.clone())
                .collect();
            for key in stale_keys {
                if let Some(e) = guard.entries.remove(&key) {
                    guard.reserved_bytes -= e.file_size;
                    paths.push(e.tmp_path);
                }
            }
            paths
        };
        for p in stale {
            if let Err(e) = fs::remove_file(&p).await {
                tracing::warn!("Failed to remove stale temp file {:?}: {e}", p);
            }
        }
    }

    /// The total file size declared when the upload was started.
    pub async fn get_file_size(&self, repo_id: &str, file_path: &str) -> Option<u64> {
        let key = (repo_id.to_string(), file_path.to_string());
        let guard = self.inner.active.read().await;
        guard.entries.get(&key).map(|e| e.file_size)
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::OnceCell;

    static MANAGER: OnceCell<TempFileManager> = OnceCell::const_new();

    async fn manager() -> &'static TempFileManager {
        MANAGER
            .get_or_init(|| async {
                let tmp =
                    std::env::temp_dir().join(format!("nanofile-temp-test-{}", Uuid::new_v4()));
                fs::create_dir_all(&tmp).await.unwrap();
                TempFileManager::new(tmp, 0, 0).await
            })
            .await
    }

    /// A fresh manager for cap tests (the shared `manager()` is unlimited).
    async fn capped_manager(max_uploads: u64, max_bytes: u64) -> TempFileManager {
        let tmp = std::env::temp_dir().join(format!("nanofile-temp-cap-{}", Uuid::new_v4()));
        fs::create_dir_all(&tmp).await.unwrap();
        TempFileManager::new(tmp, max_uploads, max_bytes).await
    }

    #[tokio::test]
    async fn write_chunk_beyond_file_size_rejected() {
        let mgr = capped_manager(0, 0).await;
        let repo = "bounds-repo";
        let path = "/f.bin";
        mgr.get_or_create(repo, path, 10).await.unwrap();

        // Writing at offset 9 with 2 bytes extends past the declared size 10.
        let result = mgr.write_chunk(repo, path, 9, b"xx").await;
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::InvalidInput)
        );

        // Writing exactly up to the declared size is still allowed.
        mgr.write_chunk(repo, path, 0, b"0123456789").await.unwrap();
        mgr.finish(repo, path).await;
    }

    #[tokio::test]
    async fn get_or_create_rejects_when_upload_count_full() {
        let mgr = capped_manager(1, 0).await;
        let repo = "count-repo";
        mgr.get_or_create(repo, "/a.txt", 10).await.unwrap();

        let result = mgr.get_or_create(repo, "/b.txt", 10).await;
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn get_or_create_rejects_when_bytes_quota_full() {
        let mgr = capped_manager(0, 100).await;
        let repo = "bytes-repo";
        mgr.get_or_create(repo, "/a.txt", 100).await.unwrap();

        let result = mgr.get_or_create(repo, "/b.txt", 1).await;
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn finish_releases_reserved_bytes() {
        let mgr = capped_manager(0, 100).await;
        let repo = "release-repo";
        mgr.get_or_create(repo, "/a.txt", 100).await.unwrap();
        mgr.finish(repo, "/a.txt").await;

        // After finishing the first upload, a new one fits the quota again.
        assert!(mgr.get_or_create(repo, "/b.txt", 100).await.is_ok());
        mgr.finish(repo, "/b.txt").await;
    }

    #[tokio::test]
    async fn test_create_and_write_chunks() {
        let mgr = manager().await;
        let repo = "test-repo-1";
        let path = "/dir/file.txt";

        let tmp = mgr.get_or_create(repo, path, 100).await.unwrap();
        assert!(tmp.exists());

        // Write first half
        mgr.write_chunk(repo, path, 0, b"hello ").await.unwrap();
        assert_eq!(mgr.get_uploaded_bytes(repo, path).await, Some(6));

        // Write second half at offset 6
        mgr.write_chunk(repo, path, 6, b"world").await.unwrap();
        assert_eq!(mgr.get_uploaded_bytes(repo, path).await, Some(11));

        // Read back
        let data = mgr.read_complete(repo, path).await.unwrap();
        assert_eq!(&data, b"hello world");

        mgr.finish(repo, path).await;
        assert!(!tmp.exists());
        assert_eq!(mgr.get_uploaded_bytes(repo, path).await, None);
    }

    #[tokio::test]
    async fn test_get_or_create_idempotent() {
        let mgr = manager().await;
        let repo = "test-repo-2";
        let path = "/readme.md";

        let a = mgr.get_or_create(repo, path, 50).await.unwrap();
        let b = mgr.get_or_create(repo, path, 50).await.unwrap();
        assert_eq!(a, b, "second call should return same path");

        mgr.finish(repo, path).await;
    }

    #[tokio::test]
    async fn test_write_chunk_before_create_returns_error() {
        let mgr = manager().await;
        let repo = "test-repo-3";
        let path = "/noexist.dat";

        let result = mgr.write_chunk(repo, path, 0, b"data").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_finish_cleans_up() {
        let mgr = manager().await;
        let repo = "test-repo-4";
        let path = "/cleanup.txt";

        let tmp = mgr.get_or_create(repo, path, 10).await.unwrap();
        mgr.write_chunk(repo, path, 0, b"12345").await.unwrap();
        assert!(tmp.exists());

        mgr.finish(repo, path).await;
        assert!(!tmp.exists(), "temp file should be deleted");
        assert_eq!(mgr.get_uploaded_bytes(repo, path).await, None);
    }

    #[tokio::test]
    async fn test_file_size() {
        let mgr = manager().await;
        let repo = "test-repo-5";
        let path = "/size_check.iso";

        mgr.get_or_create(repo, path, 999).await.unwrap();
        assert_eq!(mgr.get_file_size(repo, path).await, Some(999));

        mgr.finish(repo, path).await;
    }

    #[tokio::test]
    async fn test_read_stream_matches_complete() {
        let mgr = manager().await;
        let repo = "test-repo-stream";
        let path = "/stream.bin";

        mgr.get_or_create(repo, path, 11).await.unwrap();
        mgr.write_chunk(repo, path, 0, b"hello world")
            .await
            .unwrap();

        let mut stream = mgr.read_stream(repo, path).await.unwrap();
        use futures::stream::StreamExt;
        let mut all = Vec::new();
        while let Some(r) = stream.next().await {
            all.extend_from_slice(r.unwrap().as_ref());
        }
        assert_eq!(all, b"hello world");

        mgr.finish(repo, path).await;
    }

    // ── Streaming (in-transit CDC) tests ─────────────────────────────────

    /// Deterministic pseudo-random bytes (same generator as the `file_ops`
    /// tests) so a fixture spanning multiple CDC blocks is reproducible.
    fn pseudo_data(len: usize) -> Vec<u8> {
        let mut x: u64 = 0x9E3779B97F4A7C15;
        (0..len)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) as u8
            })
            .collect()
    }

    /// A throwaway block store backed by a temp dir.
    fn temp_store() -> (tempfile::TempDir, DynBlockStorage) {
        let dir = tempfile::tempdir().unwrap();
        let store = infra::storage::new_block_store(dir.path());
        (dir, store)
    }

    /// Chunk `data` with a fresh `Chunker` and write each block to `store`,
    /// mirroring `FileOps::write_stream_blocks`. Returns the reference ids that
    /// an in-order streaming upload must reproduce exactly.
    async fn reference_block_ids(store: &DynBlockStorage, data: &[u8]) -> Vec<String> {
        let mut chunker = Chunker::new(data.len());
        let mut ids = Vec::new();
        for blk in chunker.feed(data) {
            ids.push(store.write_block(&blk).await.unwrap());
        }
        let tail = chunker.finish();
        if !tail.is_empty() {
            ids.push(store.write_block(&tail).await.unwrap());
        }
        ids
    }

    /// In-order chunks stream straight into blocks: the produced block ids
    /// match whole-file chunking exactly, so the final chunk can commit without
    /// re-reading the temp file.
    #[tokio::test]
    async fn test_feed_stream_in_order_matches_whole_file_chunking() {
        let mgr = manager().await;
        let (_store_dir, store) = temp_store();
        let repo = "stream-repo-inorder";
        let path = "/big.bin";

        // ~6 MiB so the CDC chunker (min 256 KiB / max 4 MiB, avg ~1 MiB)
        // reliably spans multiple blocks for this pseudo-random data.
        let data = pseudo_data(6 * 1024 * 1024 + 123);
        let expected = reference_block_ids(&store, &data).await;
        assert!(expected.len() > 1, "fixture must span multiple blocks");

        mgr.get_or_create(repo, path, data.len() as u64)
            .await
            .unwrap();

        // Feed the data as 8192-byte in-order slices (like web chunk uploads).
        let mut fed = 0usize;
        while fed < data.len() {
            let end = (fed + 8192).min(data.len());
            let outcome = mgr
                .feed_stream(&store, repo, path, fed as u64, &data[fed..end])
                .await;
            assert!(matches!(outcome, FeedOutcome::Streamed { .. }));
            fed = end;
        }

        let (block_ids, total) = mgr
            .take_streamed_blocks(&store, repo, path, data.len() as u64)
            .await
            .expect("fully in-order upload should stream");
        assert_eq!(
            block_ids, expected,
            "streamed ids must match whole-file chunking"
        );
        assert_eq!(total, data.len() as i64);

        mgr.finish(repo, path).await;
    }

    /// A chunk that skips the prefix (offset > next_offset) disables streaming;
    /// the final assembly falls back to the temp file.
    #[tokio::test]
    async fn test_feed_stream_out_of_order_marks_broken() {
        let mgr = manager().await;
        let (_dir, store) = temp_store();
        let repo = "stream-repo-oorder";
        let path = "/f.bin";

        mgr.get_or_create(repo, path, 100).await.unwrap();

        let outcome = mgr.feed_stream(&store, repo, path, 50, b"0123456789").await;
        assert!(matches!(outcome, FeedOutcome::Broken));
        assert!(
            mgr.take_streamed_blocks(&store, repo, path, 100)
                .await
                .is_none(),
            "out-of-order upload must fall back to the temp file"
        );

        mgr.finish(repo, path).await;
    }

    /// Re-sending an already-fed offset (a retry) is not contiguous either and
    /// must disable streaming.
    #[tokio::test]
    async fn test_feed_stream_duplicate_chunk_marks_broken() {
        let mgr = manager().await;
        let (_dir, store) = temp_store();
        let repo = "stream-repo-dup";
        let path = "/f.bin";

        mgr.get_or_create(repo, path, 20).await.unwrap();

        let first = mgr.feed_stream(&store, repo, path, 0, b"0123456789").await;
        assert!(matches!(first, FeedOutcome::Streamed { .. }));

        let second = mgr.feed_stream(&store, repo, path, 0, b"0123456789").await;
        assert!(matches!(second, FeedOutcome::Broken));
        assert!(
            mgr.take_streamed_blocks(&store, repo, path, 20)
                .await
                .is_none()
        );

        mgr.finish(repo, path).await;
    }

    /// Once broken, further feeds keep returning `Broken` without panicking.
    #[tokio::test]
    async fn test_feed_stream_after_broken_is_stable() {
        let mgr = manager().await;
        let (_dir, store) = temp_store();
        let repo = "stream-repo-broken";
        let path = "/f.bin";

        mgr.get_or_create(repo, path, 100).await.unwrap();
        assert!(matches!(
            mgr.feed_stream(&store, repo, path, 42, b"x").await,
            FeedOutcome::Broken
        ));

        for offset in [0u64, 42, 99] {
            assert!(matches!(
                mgr.feed_stream(&store, repo, path, offset, b"y").await,
                FeedOutcome::Broken
            ));
        }
        mgr.finish(repo, path).await;
    }

    /// A block-store write failure must disable streaming (the temp file stays
    /// the source of truth), not panic.
    #[tokio::test]
    async fn test_feed_stream_block_write_failure_marks_broken() {
        let mgr = manager().await;
        // A store whose base path is a regular file: the first write's
        // `ensure_dirs` fails, so `write_block` errors.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("not-a-dir");
        std::fs::write(&file_path, b"x").unwrap();
        let store = infra::storage::new_block_store(&file_path);

        let repo = "stream-repo-wfail";
        let path = "/f.bin";
        // Larger than one max-size CDC block (4 MiB) so at least one block is
        // emitted by the first feed, forcing a block write to be attempted.
        let data = pseudo_data(4 * 1024 * 1024 + 1);

        mgr.get_or_create(repo, path, data.len() as u64)
            .await
            .unwrap();
        let outcome = mgr.feed_stream(&store, repo, path, 0, &data).await;
        assert!(
            matches!(outcome, FeedOutcome::Broken),
            "block write failure must disable streaming"
        );
        assert!(
            mgr.take_streamed_blocks(&store, repo, path, data.len() as u64)
                .await
                .is_none()
        );

        mgr.finish(repo, path).await;
    }
}
