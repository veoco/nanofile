// ─── TempFileManager ─────────────────────────────────────────────────────
//!
//! Manages temporary files for resumable/chunked uploads.
//!
//! Uses an in-memory `HashMap` to track active uploads; on server restart
//! all leftover temp files in `{temp_dir}/upload/` are wiped clean.
//!
//! Thread-safe (Arc<RwLock<...>>), designed for concurrent chunk writes.

use std::collections::HashMap;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::fs;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Manages per-upload temporary files on disk with an in-memory index.
#[derive(Clone)]
pub struct TempFileManager {
    inner: Arc<Inner>,
}

struct Inner {
    /// (repo_id, file_path_in_repo) → active temp file entry
    active: RwLock<HashMap<(String, String), TempFileEntry>>,
    /// Root directory for upload temp files, e.g. `data/temp`
    temp_dir: PathBuf,
}

struct TempFileEntry {
    tmp_path: PathBuf,
    /// Total file size as declared in the first Content-Range header
    file_size: u64,
    #[allow(dead_code)]
    created_at: Instant,
}

impl TempFileManager {
    /// Create a new manager and clean up any leftover temp files from a
    /// previous run by removing `{temp_dir}/upload/` entirely.
    pub async fn new(temp_dir: PathBuf) -> Self {
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
                active: RwLock::new(HashMap::new()),
                temp_dir,
            }),
        }
    }

    /// Return or create the temp file path for a given upload.
    /// On first call for a given (repo_id, file_path), creates a new unique
    /// temp file and records it in the in-memory index.
    pub async fn get_or_create(
        &self,
        repo_id: &str,
        file_path: &str,
        file_size: u64,
    ) -> std::io::Result<PathBuf> {
        let key = (repo_id.to_string(), file_path.to_string());
        let mut guard = self.inner.active.write().await;

        if let Some(entry) = guard.get(&key) {
            return Ok(entry.tmp_path.clone());
        }

        let dir = self.inner.temp_dir.join("upload").join(repo_id);
        fs::create_dir_all(&dir).await?;

        let tmp_path = dir.join(Uuid::new_v4().to_string());
        // Create an empty file so other chunks can open it for writing
        fs::write(&tmp_path, &[]).await?;

        guard.insert(
            key,
            TempFileEntry {
                tmp_path: tmp_path.clone(),
                file_size,
                created_at: Instant::now(),
            },
        );

        Ok(tmp_path)
    }

    /// Write `data` at `offset` into the temp file identified by
    /// (repo_id, file_path).  The file must already exist (via
    /// `get_or_create`).
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
            guard.get(&key).map(|e| e.tmp_path.clone()).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no active temp file for this upload",
                )
            })?
        };

        let data = data.to_vec(); // clone for the blocking closure
        tokio::task::spawn_blocking(move || {
            let f = std::fs::OpenOptions::new()
                .write(true)
                .create(false)
                .open(&tmp_path)?;
            f.write_all_at(&data, offset)?;
            Ok::<_, std::io::Error>(())
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))??;

        Ok(())
    }

    /// How many bytes have been written to the temp file so far?
    /// Returns `None` when no temp file exists for this upload.
    pub async fn get_uploaded_bytes(&self, repo_id: &str, file_path: &str) -> Option<u64> {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let guard = self.inner.active.read().await;
            guard.get(&key).map(|e| e.tmp_path.clone())?
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
            guard.get(&key).map(|e| e.tmp_path.clone())?
        };
        fs::read(&tmp_path).await.ok()
    }

    /// Mark an upload as finished: remove the in-memory record and delete
    /// the temporary file from disk.
    pub async fn finish(&self, repo_id: &str, file_path: &str) {
        let key = (repo_id.to_string(), file_path.to_string());
        let tmp_path = {
            let mut guard = self.inner.active.write().await;
            guard.remove(&key).map(|e| e.tmp_path)
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
            guard.remove(&key).map(|e| e.tmp_path)
        };
        if let Some(p) = tmp_path {
            let _ = fs::remove_file(&p).await;
        }
    }

    /// The total file size declared when the upload was started.
    pub async fn get_file_size(&self, repo_id: &str, file_path: &str) -> Option<u64> {
        let key = (repo_id.to_string(), file_path.to_string());
        let guard = self.inner.active.read().await;
        guard.get(&key).map(|e| e.file_size)
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
                TempFileManager::new(tmp).await
            })
            .await
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
}
