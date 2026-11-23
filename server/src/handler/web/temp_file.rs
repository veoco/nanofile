//! In-memory index over `{temp_dir}/upload/` for resumable/chunked uploads.
//!
//! Entry map and reserved-byte total share one write lock, so the quota is
//! enforced atomically with admission; leftover files are wiped on restart.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures::stream::StreamExt;
use infra::crypto::fs_id::sha1_hex;
use infra::storage::DynBlockStorage;
use infra::storage::cdc::Chunker;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

#[derive(Clone)]
pub struct TempFileManager {
    inner: Arc<Inner>,
}

/// Active temp uploads plus their declared sizes, both under one write lock so
/// the reserved-bytes quota is enforced atomically with the entry map.
struct ActiveUploads {
    /// Keyed by `(scope, repo_id, file_path)`: two credentials uploading the same
    /// path in the same library must never share a temp file or an entry.
    entries: HashMap<(String, String, String), TempFileEntry>,
    reserved_bytes: u64,
}

struct Inner {
    active: RwLock<ActiveUploads>,
    temp_dir: PathBuf,
    /// Cap on concurrent active uploads (0 = unlimited).
    max_uploads: u64,
    /// Cap on total reserved bytes across active uploads (0 = unlimited).
    max_bytes: u64,
}

/// One reaped upload: repo id, temp path, owner, upload id, new block ids.
type StaleUpload = (String, PathBuf, Option<i32>, Uuid, Vec<String>);

struct TempFileEntry {
    tmp_path: PathBuf,
    /// Stable identity for this upload's quota reservation. Retrying the first
    /// chunk reuses it, so the declared size is reserved once.
    upload_id: Uuid,
    /// Total file size as declared in the first Content-Range header
    file_size: u64,
    created_at: Instant,
    /// Uploading user, once the caller binds it; used to release the block
    /// reservation when the upload is abandoned.
    owner: Option<i32>,
    /// Blocks written by this upload that no commit references yet, so
    /// `abort`/`cleanup_stale` can delete them instead of leaving orphans.
    new_block_ids: Vec<String>,
    /// Per-upload CDC streaming state; see `feed_stream`.
    stream: Arc<Mutex<Option<UploadStream>>>,
}

/// Incremental CDC state for one resumable upload. An out-of-order chunk or a
/// block-write failure flips `broken`, after which the upload falls back to the
/// plain temp-file assembly path.
struct UploadStream {
    chunker: Option<Chunker>,
    /// Bytes successfully fed into `chunker` so far (the in-order prefix).
    next_offset: u64,
    block_ids: Vec<String>,
    /// Block ids that were **newly written** by this upload (not deduped), so
    /// quota failures can clean them up without deleting shared blocks.
    new_block_ids: Vec<String>,
    /// `(block_id, logical length)` for every block the streaming path wrote, in
    /// order; the fast commit re-checks these against the store and falls back
    /// to assembling the temp file if one is missing or shorter than streamed.
    streamed_blocks: Vec<(String, u64)>,
    /// Declared file size, copied from the entry so the stream is self-contained.
    file_size: u64,
    /// Set on the first out-of-order chunk or block-write failure.
    broken: bool,
}

/// Outcome of feeding one chunk to an upload's streaming CDC state.
pub enum FeedOutcome {
    /// The chunk was in order and its completed blocks were persisted.
    Streamed {
        block_ids: Vec<String>,
        /// Block ids that were newly written (not deduped) by this feed.
        new_block_ids: Vec<String>,
    },
    /// The chunk was out of order or a write failed; streaming is disabled.
    Broken,
}

/// Filesystem-safe subdirectory name for a repo's in-progress uploads.
///
/// `repo_id` is client input (`../../../../tmp/x` reaches here), so it must not be
/// used as a path segment; hashing keeps uploads separated without path semantics.
fn repo_dir_name(repo_id: &str) -> String {
    infra::crypto::fs_id::sha1_hex(repo_id.as_bytes())
}

/// Composite key of an in-progress upload: credential scope, repo and path.
///
/// The scope keeps two different credentials (two users, or two upload-link
/// visitors minted from the same link) apart even when they target the same
/// repo and path; without it the second caller would be handed the first one's
/// temp file and the two uploads would corrupt each other.
fn entry_key(scope: &str, repo_id: &str, file_path: &str) -> (String, String, String) {
    (
        scope.to_string(),
        repo_id.to_string(),
        file_path.to_string(),
    )
}

/// Delete blocks an abandoned upload wrote and no commit references.
///
/// Best effort, and only blocks this upload newly wrote are tracked; a concurrent
/// identical upload may have committed one since, so each candidate is re-checked
/// against the FS objects and kept when referenced or when the check fails.
async fn delete_uncommitted_blocks(
    repos: &crate::repository::Repositories,
    store: &DynBlockStorage,
    repo_id: &str,
    ids: &[String],
) {
    for id in ids {
        match repos.fs_object.references_block(repo_id, id).await {
            Ok(true) => {
                tracing::debug!("keeping block {id} in repo {repo_id}: a file references it");
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                // Unknown: keep the block rather than risk deleting data.
                tracing::warn!("could not check references for block {id}: {e}");
                continue;
            }
        }
        if let Err(e) = store.remove_block(repo_id, id).await {
            tracing::warn!("failed to remove abandoned block {id} in repo {repo_id}: {e}");
        }
    }
    // The abandoned upload no longer needs its restart guard for these blocks; a
    // row that survives is cleared by the expiry sweep after it re-checks
    // references, so nothing a commit references can be deleted here.
    if let Err(e) = repos.pending_block.forget(repo_id, ids).await {
        tracing::warn!(
            repo_id,
            "could not clear pending-block rows of an abandoned upload: {e}"
        );
    }
}

impl TempFileManager {
    /// Create a new manager, wiping `{temp_dir}/upload/` from a previous run.
    /// `max_uploads`/`max_bytes` bound active uploads and their declared sizes;
    /// `0` disables each limit.
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
    ///
    /// Rejects with `ErrorKind::QuotaExceeded` past the configured active-upload
    /// count or reserved-bytes cap, so an attacker (e.g. an anonymous upload link)
    /// cannot pile up unbounded temp files or entries by starting abandoned uploads.
    pub async fn get_or_create(
        &self,
        scope: &str,
        repo_id: &str,
        file_path: &str,
        file_size: u64,
    ) -> std::io::Result<PathBuf> {
        let key = entry_key(scope, repo_id, file_path);
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

        let dir = self
            .inner
            .temp_dir
            .join("upload")
            .join(repo_dir_name(repo_id));
        fs::create_dir_all(&dir).await?;

        let tmp_path = dir.join(Uuid::new_v4().to_string());
        // Defence in depth: `repo_dir_name` has removed all path semantics, but
        // the invariant is asserted anyway so a refactor cannot reintroduce a traversal.
        debug_assert!(
            tmp_path.starts_with(&self.inner.temp_dir),
            "temp upload path escaped temp_dir: {tmp_path:?}"
        );
        fs::write(&tmp_path, &[]).await?;

        guard.reserved_bytes += file_size;
        let upload_id = Uuid::new_v4();
        guard.entries.insert(
            key,
            TempFileEntry {
                tmp_path: tmp_path.clone(),
                upload_id,
                file_size,
                created_at: Instant::now(),
                owner: None,
                new_block_ids: Vec::new(),
                stream: Arc::new(Mutex::new(Some(UploadStream {
                    chunker: None,
                    next_offset: 0,
                    block_ids: Vec::new(),
                    new_block_ids: Vec::new(),
                    streamed_blocks: Vec::new(),
                    file_size,
                    broken: false,
                }))),
            },
        );

        Ok(tmp_path)
    }

    pub async fn bind_owner(&self, scope: &str, repo_id: &str, file_path: &str, user_id: i32) {
        let key = entry_key(scope, repo_id, file_path);
        let mut guard = self.inner.active.write().await;
        if let Some(entry) = guard.entries.get_mut(&key) {
            entry.owner = Some(user_id);
        }
    }

    pub async fn record_new_blocks(
        &self,
        scope: &str,
        repo_id: &str,
        file_path: &str,
        ids: &[String],
    ) {
        if ids.is_empty() {
            return;
        }
        let key = entry_key(scope, repo_id, file_path);
        let mut guard = self.inner.active.write().await;
        if let Some(entry) = guard.entries.get_mut(&key) {
            entry.new_block_ids.extend(ids.iter().cloned());
        }
    }

    /// Write `data` at `offset` into the temp file for (repo_id, file_path); the
    /// file must already exist (via `get_or_create`).
    ///
    /// Rejects with `ErrorKind::InvalidInput` when the chunk would extend past the
    /// declared size — otherwise a client could create a huge sparse file at an
    /// arbitrary offset.
    pub async fn write_chunk(
        &self,
        scope: &str,
        repo_id: &str,
        file_path: &str,
        offset: u64,
        data: bytes::Bytes,
    ) -> std::io::Result<()> {
        let key = entry_key(scope, repo_id, file_path);
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

        // The blocking closure needs an owned value; `Bytes` makes that a
        // refcount bump rather than a second copy of the chunk.
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

    /// Stream an in-order chunk through the upload's CDC chunker and persist any
    /// completed blocks, so a fully in-order upload can commit on the final chunk
    /// without re-reading the assembled temp file.
    ///
    /// The chunk is only consumed when strictly contiguous with the streamed
    /// prefix (`offset == next_offset`); any other chunk disables streaming, and
    /// the always-written temp file remains the fallback.
    pub async fn feed_stream(
        &self,
        store: &DynBlockStorage,
        scope: &str,
        repo_id: &str,
        file_path: &str,
        offset: u64,
        data: &[u8],
    ) -> FeedOutcome {
        // Read the stream handle under the map lock, then release it before
        // the long-running stream lock + block writes (no lock nesting).
        let stream_handle = {
            let guard = self.inner.active.read().await;
            match guard.entries.get(&entry_key(scope, repo_id, file_path)) {
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
        let mut new_ids = Vec::new();
        let mut streamed = Vec::new();
        for blk in chunker.feed(data) {
            let block_id = sha1_hex(&blk);
            let len = blk.len() as u64;
            match store
                .write_block_with_id_tracked(repo_id, &block_id, &blk)
                .await
            {
                Ok((id, was_new)) => {
                    streamed.push((id.clone(), len));
                    ids.push(id.clone());
                    if was_new {
                        new_ids.push(id);
                    }
                }
                Err(_) => {
                    // The chunker advanced but the block is lost; disable streaming.
                    state.broken = true;
                    return FeedOutcome::Broken;
                }
            }
        }
        state.block_ids.extend(ids.iter().cloned());
        state.new_block_ids.extend(new_ids.iter().cloned());
        state.streamed_blocks.extend(streamed);
        state.next_offset += data.len() as u64;
        FeedOutcome::Streamed {
            block_ids: ids,
            new_block_ids: new_ids,
        }
    }

    /// Consume the fully-streamed block ids for an upload, if it streamed the whole
    /// file in order.
    ///
    /// Returns `Some((block_ids, total_size, new_block_ids))` only when the stream
    /// is intact and covered the entire declared file; `new_block_ids` is the subset
    /// this upload newly wrote (so quota failures can clean it up), and
    /// `expected_size` is a defensive cross-check against a client changing
    /// `file_size` mid-upload. Otherwise returns `None` and the caller falls back to
    /// reading the temp file.
    pub async fn take_streamed_blocks(
        &self,
        store: &DynBlockStorage,
        scope: &str,
        repo_id: &str,
        file_path: &str,
        expected_size: u64,
    ) -> Option<(Vec<String>, i64, Vec<String>)> {
        let stream_handle = {
            let guard = self.inner.active.read().await;
            guard
                .entries
                .get(&entry_key(scope, repo_id, file_path))
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
            let block_id = sha1_hex(&tail);
            let len = tail.len() as u64;
            match store
                .write_block_with_id_tracked(repo_id, &block_id, &tail)
                .await
            {
                Ok((id, was_new)) => {
                    if was_new {
                        state.new_block_ids.push(id.clone());
                    }
                    state.streamed_blocks.push((id.clone(), len));
                    state.block_ids.push(id);
                }
                Err(_) => return None,
            }
        }

        // The fast path skips temp-file assembly, so it must confirm what streaming
        // wrote is still intact: every block present at the streamed length and the
        // streamed bytes covering the whole file, else fall back to the temp file.
        let streamed_len: u64 = state.streamed_blocks.iter().map(|(_, len)| *len).sum();
        if streamed_len != state.file_size {
            tracing::warn!(
                "streamed {streamed_len} bytes but the upload declared {}; \
                 falling back to the assembled temp file",
                state.file_size
            );
            return None;
        }
        for (id, expected) in &state.streamed_blocks {
            match store.block_size(repo_id, id).await {
                Ok(size) if size as u64 == *expected => {}
                Ok(size) => {
                    tracing::warn!(
                        "streamed block {id} is {size} bytes on disk, expected {expected}; \
                         falling back to the assembled temp file"
                    );
                    return None;
                }
                Err(e) => {
                    tracing::warn!(
                        "streamed block {id} is unreadable ({e}); \
                         falling back to the assembled temp file"
                    );
                    return None;
                }
            }
        }

        Some((
            std::mem::take(&mut state.block_ids),
            state.file_size as i64,
            std::mem::take(&mut state.new_block_ids),
        ))
    }

    /// Bytes written to the temp file so far (`None` when no temp file exists).
    pub async fn get_uploaded_bytes(
        &self,
        scope: &str,
        repo_id: &str,
        file_path: &str,
    ) -> Option<u64> {
        let key = entry_key(scope, repo_id, file_path);
        let tmp_path = {
            let guard = self.inner.active.read().await;
            guard.entries.get(&key).map(|e| e.tmp_path.clone())?
        };
        match fs::metadata(&tmp_path).await {
            Ok(m) => Some(m.len()),
            Err(_) => None,
        }
    }

    /// Read the complete temp file into memory; only valid once the last chunk has
    /// been written (the file is fully assembled).
    pub async fn read_complete(
        &self,
        scope: &str,
        repo_id: &str,
        file_path: &str,
    ) -> Option<Vec<u8>> {
        let key = entry_key(scope, repo_id, file_path);
        let tmp_path = {
            let guard = self.inner.active.read().await;
            guard.entries.get(&key).map(|e| e.tmp_path.clone())?
        };
        fs::read(&tmp_path).await.ok()
    }

    /// Stream the assembled temp file in bounded chunks, so the assembly path never
    /// reads the whole file into memory. `None` when no active temp file exists.
    pub async fn read_stream(
        &self,
        scope: &str,
        repo_id: &str,
        file_path: &str,
    ) -> Option<futures::stream::BoxStream<'static, std::io::Result<bytes::Bytes>>> {
        let key = entry_key(scope, repo_id, file_path);
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

    /// Remove the temp entry and return `(owner, upload_id)` so the caller can
    /// release that upload's quota reservation. The blocks were committed, so the
    /// reservation itself is what must go; the blocks are now counted by `repo.size`.
    pub async fn finish(
        &self,
        scope: &str,
        repo_id: &str,
        file_path: &str,
    ) -> Option<(Option<i32>, Uuid)> {
        let key = entry_key(scope, repo_id, file_path);
        let (tmp_path, reservation) = {
            let mut guard = self.inner.active.write().await;
            let removed = guard.entries.remove(&key);
            if let Some(e) = &removed {
                guard.reserved_bytes -= e.file_size;
            }
            match removed {
                Some(e) => (Some(e.tmp_path), Some((e.owner, e.upload_id))),
                None => (None, None),
            }
        };
        if let Some(p) = tmp_path {
            let _ = fs::remove_file(&p).await;
        }
        reservation
    }

    /// The reservation id of an existing temp upload.
    pub async fn upload_id(&self, scope: &str, repo_id: &str, file_path: &str) -> Option<Uuid> {
        let key = entry_key(scope, repo_id, file_path);
        let guard = self.inner.active.read().await;
        guard.entries.get(&key).map(|e| e.upload_id)
    }

    /// Abort an upload: remove its temp file, delete the blocks it wrote that no
    /// commit references, and release its block reservation.
    ///
    /// Clients have no protocol-level abort (a cancelled upload is purely local to
    /// seaf-daemon and the Qt client), so this is only reached from nanofile's own
    /// failure paths; [`Self::cleanup_stale`] covers clients that stop sending chunks.
    pub async fn abort(
        &self,
        repos: &crate::repository::Repositories,
        scope: &str,
        repo_id: &str,
        file_path: &str,
        store: &DynBlockStorage,
    ) {
        let key = entry_key(scope, repo_id, file_path);
        let (tmp_path, owner, upload_id, blocks) = {
            let mut guard = self.inner.active.write().await;
            let removed = guard.entries.remove(&key);
            if let Some(e) = &removed {
                guard.reserved_bytes -= e.file_size;
            }
            match removed {
                Some(e) => (
                    Some(e.tmp_path),
                    e.owner,
                    Some(e.upload_id),
                    e.new_block_ids,
                ),
                None => (None, None, None, Vec::new()),
            }
        };
        if let Some(p) = tmp_path {
            let _ = fs::remove_file(&p).await;
        }
        delete_uncommitted_blocks(repos, store, repo_id, &blocks).await;
        if let (Some(uid), Some(upload_id)) = (owner, upload_id) {
            crate::service::fs::quota::release_upload(repos, uid, repo_id, &upload_id.to_string());
        }
    }

    /// Drop uploads idle longer than `ttl`, deleting their temp files and any
    /// blocks they wrote that no commit references, and releasing their block
    /// reservations. Called periodically so abandoned resumable uploads don't leak
    /// memory, disk or quota.
    ///
    /// Returns how many uploads were dropped.
    pub async fn cleanup_stale(
        &self,
        repos: &crate::repository::Repositories,
        ttl: std::time::Duration,
        store: &DynBlockStorage,
    ) -> usize {
        let cutoff = Instant::now() - ttl;
        let stale: Vec<StaleUpload> = {
            let mut guard = self.inner.active.write().await;
            let mut removed = Vec::new();
            // Collect keys first so the closure doesn't borrow `entries` and
            // `reserved_bytes` from `guard` at once.
            let stale_keys: Vec<(String, String, String)> = guard
                .entries
                .iter()
                .filter(|(_, e)| e.created_at < cutoff)
                .map(|(k, _)| k.clone())
                .collect();
            for key in stale_keys {
                if let Some(e) = guard.entries.remove(&key) {
                    guard.reserved_bytes -= e.file_size;
                    // The key carries the repo id; the on-disk dir name is only a
                    // hash of it, so it cannot be recovered from the path. The
                    // scope is not needed once the entry is reaped.
                    removed.push((key.1, e.tmp_path, e.owner, e.upload_id, e.new_block_ids));
                }
            }
            removed
        };
        let dropped = stale.len();
        for (repo_id, p, owner, upload_id, blocks) in stale {
            if let Err(e) = fs::remove_file(&p).await {
                tracing::warn!("Failed to remove stale temp file {:?}: {e}", p);
            }
            delete_uncommitted_blocks(repos, store, &repo_id, &blocks).await;
            if let Some(uid) = owner {
                crate::service::fs::quota::release_upload(
                    repos,
                    uid,
                    &repo_id,
                    &upload_id.to_string(),
                );
            }
        }
        dropped
    }

    pub async fn get_file_size(&self, scope: &str, repo_id: &str, file_path: &str) -> Option<u64> {
        let key = entry_key(scope, repo_id, file_path);
        let guard = self.inner.active.read().await;
        guard.entries.get(&key).map(|e| e.file_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::OnceCell;

    /// Default credential scope for unit tests; isolation tests use their own.
    const SCOPE: &str = "user:1";

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

    /// A chunk as the blocking writer receives it (refcounted, no copy).
    fn chunk(data: &'static [u8]) -> bytes::Bytes {
        bytes::Bytes::from_static(data)
    }

    #[tokio::test]
    async fn write_chunk_beyond_file_size_rejected() {
        let mgr = capped_manager(0, 0).await;
        let repo = "bounds-repo";
        let path = "/f.bin";
        mgr.get_or_create(SCOPE, repo, path, 10).await.unwrap();

        let result = mgr.write_chunk(SCOPE, repo, path, 9, chunk(b"xx")).await;
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::InvalidInput)
        );

        // Writing exactly up to the declared size is still allowed.
        mgr.write_chunk(SCOPE, repo, path, 0, chunk(b"0123456789"))
            .await
            .unwrap();
        mgr.finish(SCOPE, repo, path).await;
    }

    #[tokio::test]
    async fn get_or_create_rejects_when_upload_count_full() {
        let mgr = capped_manager(1, 0).await;
        let repo = "count-repo";
        mgr.get_or_create(SCOPE, repo, "/a.txt", 10).await.unwrap();

        let result = mgr.get_or_create(SCOPE, repo, "/b.txt", 10).await;
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn get_or_create_rejects_when_bytes_quota_full() {
        let mgr = capped_manager(0, 100).await;
        let repo = "bytes-repo";
        mgr.get_or_create(SCOPE, repo, "/a.txt", 100).await.unwrap();

        let result = mgr.get_or_create(SCOPE, repo, "/b.txt", 1).await;
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn finish_releases_reserved_bytes() {
        let mgr = capped_manager(0, 100).await;
        let repo = "release-repo";
        mgr.get_or_create(SCOPE, repo, "/a.txt", 100).await.unwrap();
        mgr.finish(SCOPE, repo, "/a.txt").await;

        assert!(mgr.get_or_create(SCOPE, repo, "/b.txt", 100).await.is_ok());
        mgr.finish(SCOPE, repo, "/b.txt").await;
    }

    #[tokio::test]
    async fn test_create_and_write_chunks() {
        let mgr = manager().await;
        let repo = "test-repo-1";
        let path = "/dir/file.txt";

        let tmp = mgr.get_or_create(SCOPE, repo, path, 100).await.unwrap();
        assert!(tmp.exists());

        mgr.write_chunk(SCOPE, repo, path, 0, chunk(b"hello "))
            .await
            .unwrap();
        assert_eq!(mgr.get_uploaded_bytes(SCOPE, repo, path).await, Some(6));

        mgr.write_chunk(SCOPE, repo, path, 6, chunk(b"world"))
            .await
            .unwrap();
        assert_eq!(mgr.get_uploaded_bytes(SCOPE, repo, path).await, Some(11));

        let data = mgr.read_complete(SCOPE, repo, path).await.unwrap();
        assert_eq!(&data, b"hello world");

        mgr.finish(SCOPE, repo, path).await;
        assert!(!tmp.exists());
        assert_eq!(mgr.get_uploaded_bytes(SCOPE, repo, path).await, None);
    }

    #[tokio::test]
    async fn test_get_or_create_idempotent() {
        let mgr = manager().await;
        let repo = "test-repo-2";
        let path = "/readme.md";

        let a = mgr.get_or_create(SCOPE, repo, path, 50).await.unwrap();
        let b = mgr.get_or_create(SCOPE, repo, path, 50).await.unwrap();
        assert_eq!(a, b, "second call should return same path");

        mgr.finish(SCOPE, repo, path).await;
    }

    #[tokio::test]
    async fn test_write_chunk_before_create_returns_error() {
        let mgr = manager().await;
        let repo = "test-repo-3";
        let path = "/noexist.dat";

        let result = mgr.write_chunk(SCOPE, repo, path, 0, chunk(b"data")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_finish_cleans_up() {
        let mgr = manager().await;
        let repo = "test-repo-4";
        let path = "/cleanup.txt";

        let tmp = mgr.get_or_create(SCOPE, repo, path, 10).await.unwrap();
        mgr.write_chunk(SCOPE, repo, path, 0, chunk(b"12345"))
            .await
            .unwrap();
        assert!(tmp.exists());

        mgr.finish(SCOPE, repo, path).await;
        assert!(!tmp.exists(), "temp file should be deleted");
        assert_eq!(mgr.get_uploaded_bytes(SCOPE, repo, path).await, None);
    }

    #[tokio::test]
    async fn test_file_size() {
        let mgr = manager().await;
        let repo = "test-repo-5";
        let path = "/size_check.iso";

        mgr.get_or_create(SCOPE, repo, path, 999).await.unwrap();
        assert_eq!(mgr.get_file_size(SCOPE, repo, path).await, Some(999));

        mgr.finish(SCOPE, repo, path).await;
    }

    /// Two credentials uploading the same repo and path get independent temp
    /// files: neither sees the other's bytes and finishing one leaves the other
    /// resumable.
    #[tokio::test]
    async fn test_scope_isolates_same_repo_and_path() {
        let mgr = manager().await;
        let repo = "scope-repo";
        let path = "/same.bin";

        let a = mgr.get_or_create("user:1", repo, path, 10).await.unwrap();
        let b = mgr.get_or_create("user:2", repo, path, 10).await.unwrap();
        assert_ne!(
            a, b,
            "different scopes must not share a temp file for the same repo/path"
        );

        mgr.write_chunk("user:1", repo, path, 0, chunk(b"AAAAA"))
            .await
            .unwrap();
        mgr.write_chunk("user:2", repo, path, 0, chunk(b"BBBBB"))
            .await
            .unwrap();

        assert_eq!(mgr.get_uploaded_bytes("user:1", repo, path).await, Some(5));
        assert_eq!(mgr.get_uploaded_bytes("user:2", repo, path).await, Some(5));
        let one = mgr.read_complete("user:1", repo, path).await.unwrap();
        let two = mgr.read_complete("user:2", repo, path).await.unwrap();
        assert_eq!(&one, b"AAAAA");
        assert_eq!(&two, b"BBBBB");

        // Finishing one upload must not remove or release the other's entry.
        assert!(mgr.finish("user:1", repo, path).await.is_some());
        assert!(mgr.upload_id("user:2", repo, path).await.is_some());
        assert!(!a.exists(), "the finished scope's temp file should be gone");
        assert!(b.exists(), "the other scope's temp file must survive");
        assert!(mgr.finish("user:1", repo, path).await.is_none());
        mgr.finish("user:2", repo, path).await;
    }

    #[tokio::test]
    async fn test_read_stream_matches_complete() {
        let mgr = manager().await;
        let repo = "test-repo-stream";
        let path = "/stream.bin";

        mgr.get_or_create(SCOPE, repo, path, 11).await.unwrap();
        mgr.write_chunk(SCOPE, repo, path, 0, chunk(b"hello world"))
            .await
            .unwrap();

        let mut stream = mgr.read_stream(SCOPE, repo, path).await.unwrap();
        use futures::stream::StreamExt;
        let mut all = Vec::new();
        while let Some(r) = stream.next().await {
            all.extend_from_slice(r.unwrap().as_ref());
        }
        assert_eq!(all, b"hello world");

        mgr.finish(SCOPE, repo, path).await;
    }

    /// Deterministic pseudo-random bytes, matching the `file_ops` test generator.
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

    fn temp_store() -> (tempfile::TempDir, DynBlockStorage) {
        let dir = tempfile::tempdir().unwrap();
        let store = infra::storage::new_block_store(dir.path());
        (dir, store)
    }

    /// Chunk `data` and store each block, mirroring `FileOps::write_stream_blocks`;
    /// returns the ids an in-order streaming upload must reproduce exactly.
    async fn reference_block_ids(store: &DynBlockStorage, data: &[u8]) -> Vec<String> {
        const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";
        let mut chunker = Chunker::new(data.len());
        let mut ids = Vec::new();
        for blk in chunker.feed(data) {
            ids.push(store.write_block(REPO, &blk).await.unwrap());
        }
        let tail = chunker.finish();
        if !tail.is_empty() {
            ids.push(store.write_block(REPO, &tail).await.unwrap());
        }
        ids
    }

    /// In-order chunks yield exactly the whole-file block ids, so the final chunk
    /// commits without re-reading the temp file.
    #[tokio::test]
    async fn test_feed_stream_in_order_matches_whole_file_chunking() {
        let mgr = manager().await;
        let (_store_dir, store) = temp_store();
        let repo = "stream-repo-inorder";
        let path = "/big.bin";

        // 12 MiB so the CDC chunker (seafile defaults: min 6 MiB / max 10 MiB /
        // avg 8 MiB) reliably spans multiple blocks for this pseudo-random data.
        let data = pseudo_data(12 * 1024 * 1024 + 123);
        let expected = reference_block_ids(&store, &data).await;
        assert!(expected.len() > 1, "fixture must span multiple blocks");

        mgr.get_or_create(SCOPE, repo, path, data.len() as u64)
            .await
            .unwrap();

        let mut fed = 0usize;
        while fed < data.len() {
            let end = (fed + 8192).min(data.len());
            let outcome = mgr
                .feed_stream(&store, SCOPE, repo, path, fed as u64, &data[fed..end])
                .await;
            assert!(matches!(outcome, FeedOutcome::Streamed { .. }));
            fed = end;
        }

        let (block_ids, total, _new_ids) = mgr
            .take_streamed_blocks(&store, SCOPE, repo, path, data.len() as u64)
            .await
            .expect("fully in-order upload should stream");
        assert_eq!(
            block_ids, expected,
            "streamed ids must match whole-file chunking"
        );
        assert_eq!(total, data.len() as i64);

        mgr.finish(SCOPE, repo, path).await;
    }

    /// A chunk that skips the prefix disables streaming; assembly falls back to the temp file.
    #[tokio::test]
    async fn test_feed_stream_out_of_order_marks_broken() {
        let mgr = manager().await;
        let (_dir, store) = temp_store();
        let repo = "stream-repo-oorder";
        let path = "/f.bin";

        mgr.get_or_create(SCOPE, repo, path, 100).await.unwrap();

        let outcome = mgr
            .feed_stream(&store, SCOPE, repo, path, 50, b"0123456789")
            .await;
        assert!(matches!(outcome, FeedOutcome::Broken));
        assert!(
            mgr.take_streamed_blocks(&store, SCOPE, repo, path, 100)
                .await
                .is_none(),
            "out-of-order upload must fall back to the temp file"
        );

        mgr.finish(SCOPE, repo, path).await;
    }

    /// Re-sending an already-fed offset (a retry) is not contiguous and disables streaming.
    #[tokio::test]
    async fn test_feed_stream_duplicate_chunk_marks_broken() {
        let mgr = manager().await;
        let (_dir, store) = temp_store();
        let repo = "stream-repo-dup";
        let path = "/f.bin";

        mgr.get_or_create(SCOPE, repo, path, 20).await.unwrap();

        let first = mgr
            .feed_stream(&store, SCOPE, repo, path, 0, b"0123456789")
            .await;
        assert!(matches!(first, FeedOutcome::Streamed { .. }));

        let second = mgr
            .feed_stream(&store, SCOPE, repo, path, 0, b"0123456789")
            .await;
        assert!(matches!(second, FeedOutcome::Broken));
        assert!(
            mgr.take_streamed_blocks(&store, SCOPE, repo, path, 20)
                .await
                .is_none()
        );

        mgr.finish(SCOPE, repo, path).await;
    }

    /// Once broken, further feeds keep returning `Broken` without panicking.
    #[tokio::test]
    async fn test_feed_stream_after_broken_is_stable() {
        let mgr = manager().await;
        let (_dir, store) = temp_store();
        let repo = "stream-repo-broken";
        let path = "/f.bin";

        mgr.get_or_create(SCOPE, repo, path, 100).await.unwrap();
        assert!(matches!(
            mgr.feed_stream(&store, SCOPE, repo, path, 42, b"x").await,
            FeedOutcome::Broken
        ));

        for offset in [0u64, 42, 99] {
            assert!(matches!(
                mgr.feed_stream(&store, SCOPE, repo, path, offset, b"y")
                    .await,
                FeedOutcome::Broken
            ));
        }
        mgr.finish(SCOPE, repo, path).await;
    }

    /// A block-store write failure disables streaming (the temp file stays the
    /// source of truth) instead of panicking.
    #[tokio::test]
    async fn test_feed_stream_block_write_failure_marks_broken() {
        let mgr = manager().await;
        // A store whose base path is a regular file: the first write's `ensure_dirs` fails.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("not-a-dir");
        std::fs::write(&file_path, b"x").unwrap();
        let store = infra::storage::new_block_store(&file_path);

        let repo = "stream-repo-wfail";
        let path = "/f.bin";
        // Larger than one max-size CDC block (seafile default 10 MiB) so the first
        // feed emits a block and attempts a write.
        let data = pseudo_data(10 * 1024 * 1024 + 1);

        mgr.get_or_create(SCOPE, repo, path, data.len() as u64)
            .await
            .unwrap();
        let outcome = mgr.feed_stream(&store, SCOPE, repo, path, 0, &data).await;
        assert!(
            matches!(outcome, FeedOutcome::Broken),
            "block write failure must disable streaming"
        );
        assert!(
            mgr.take_streamed_blocks(&store, SCOPE, repo, path, data.len() as u64)
                .await
                .is_none()
        );

        mgr.finish(SCOPE, repo, path).await;
    }
}
