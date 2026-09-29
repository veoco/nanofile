use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::crypto::fs_id::sha1_hex;
use crate::storage::{BlockStorageBackend, LegacyCopyOutcome, MAX_BLOCK_PREFIX_LEN};

/// Subdirectory of the block root that holds the per-repository directories.
///
/// Keeping the new layout under a single well-known name makes it trivially
/// distinguishable from the legacy flat layout (`{base}/<2hex>/<block_id>`) that
/// the one-shot migration has to remove, and gives the migration a definite end
/// state to verify.
pub const REPOS_DIR: &str = "repos";

/// Marker file written inside every repository directory, holding the raw
/// repository id. The directory name is a hash, so this is what maps a
/// directory back to a repository for the GC orphan sweep and for operators.
pub const REPO_MARKER_FILE: &str = ".repo_id";

/// Marker file written in the block root once the layout migration completed.
pub const LAYOUT_MARKER_FILE: &str = ".block_layout";

/// Contents of [`LAYOUT_MARKER_FILE`].
pub const LAYOUT_VERSION: &str = "v2";

/// Upper bound on the number of block ids held in the existence cache. Blocks
/// are content-addressed and (except under GC) immutable, so a large common
/// block id set is very common; this cap bounds memory when it grows unbounded.
const EXISTS_CACHE_CAPACITY: usize = 100_000;

/// Time-to-live for a cached "block exists" entry. Only blocks removed by GC or
/// `remove_block` stop existing, and GC runs far less often (24h by default),
/// so a short TTL is enough to bound any false-positive window.
const EXISTS_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct BlockStorage {
    base_dir: PathBuf,
    /// Repository directories whose 256 prefix directories and marker file have
    /// already been created, so the steady-state write path does no mkdir work.
    prepared_repos: Mutex<HashSet<String>>,
    /// Cache of recently-confirmed-existing `(repo_id, block_id)` pairs →
    /// (confirmation time, stored length). Content addressing makes blocks
    /// immutable, so a presence result never goes stale except under
    /// `remove_block`/GC; the TTL and capacity bound the worst case. The length
    /// is what lets the write paths tell "already stored with exactly these
    /// bytes" from "a partial write left by an interrupted one", which must be
    /// replaced rather than treated as a dedup hit. Kept behind a short-lived
    /// `Mutex` lock that is never held across an `.await`, so it cannot block
    /// the runtime.
    exists_cache: Mutex<HashMap<String, (Instant, u64)>>,
}

impl BlockStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            prepared_repos: Mutex::new(HashSet::new()),
            exists_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Root directory holding every block, i.e. the configured `block_dir`.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Directory holding one repository's blocks. The repository id is hashed
    /// rather than used verbatim: it reaches this module from URL segments and
    /// multipart fields (see `repo_dir_name` in the web upload module), so it
    /// must never carry path semantics.
    pub fn repo_dir(&self, repo_id: &str) -> PathBuf {
        self.base_dir
            .join(REPOS_DIR)
            .join(sha1_hex(repo_id.as_bytes()))
    }

    /// Block IDs are content-addressed SHA-1 hashes: exactly 40 lowercase hex.
    fn is_valid_block_id(block_id: &str) -> bool {
        block_id.len() == 40 && block_id.bytes().all(|b| b.is_ascii_hexdigit())
    }

    fn block_path(&self, repo_id: &str, block_id: &str) -> PathBuf {
        // Defensive: never index before checking length (callers validate via
        // `is_valid_block_id`, but write paths also route through here).
        let prefix = block_id.get(..2).unwrap_or(block_id);
        self.repo_dir(repo_id).join(prefix).join(block_id)
    }

    /// Composite cache key for the presence cache.
    fn cache_key(repo_id: &str, block_id: &str) -> String {
        let mut key = String::with_capacity(repo_id.len() + block_id.len() + 1);
        key.push_str(repo_id);
        key.push('\u{0}');
        key.push_str(block_id);
        key
    }

    /// Drop entries older than [`EXISTS_CACHE_TTL`]. Called under the cache lock;
    /// `elapsed()` (not `duration_since`) so a never-set entry cannot panic.
    fn evict_expired(cache: &mut HashMap<String, (Instant, u64)>) {
        cache.retain(|_, (t, _)| t.elapsed() < EXISTS_CACHE_TTL);
    }

    /// Stored length of a freshly-confirmed block, or `None` when the cache
    /// holds no (fresh) entry for it.
    fn exists_cache_len(&self, repo_id: &str, block_id: &str) -> Option<u64> {
        let key = Self::cache_key(repo_id, block_id);
        let mut cache = self.exists_cache.lock().unwrap();
        Self::evict_expired(&mut cache);
        cache.get(&key).map(|(_, len)| *len)
    }

    fn exists_cache_contains(&self, repo_id: &str, block_id: &str) -> bool {
        self.exists_cache_len(repo_id, block_id).is_some()
    }

    fn exists_cache_insert(&self, repo_id: &str, block_id: &str, stored_len: u64) {
        let key = Self::cache_key(repo_id, block_id);
        let mut cache = self.exists_cache.lock().unwrap();
        Self::evict_expired(&mut cache);
        if cache.len() >= EXISTS_CACHE_CAPACITY {
            // Capacity reached with all entries still fresh: drop the whole
            // cache. Cheaper than an LRU and only costs a few extra stats.
            cache.clear();
        }
        cache.insert(key, (Instant::now(), stored_len));
    }

    fn exists_cache_remove(&self, repo_id: &str, block_id: &str) {
        let key = Self::cache_key(repo_id, block_id);
        self.exists_cache.lock().unwrap().remove(&key);
    }

    /// Create a repository's directory, its 256 prefix directories and its
    /// `.repo_id` marker, at most once per process.
    ///
    /// `create_dir_all` is idempotent and the prefix set is fixed, so a race
    /// between two writers of the same repository is harmless; the in-memory
    /// set only exists to keep the steady-state write path free of mkdir work.
    async fn ensure_repo_dirs(&self, repo_id: &str) -> io::Result<()> {
        {
            let prepared = self.prepared_repos.lock().unwrap();
            if prepared.contains(repo_id) {
                return Ok(());
            }
        }

        let repo_dir = self.repo_dir(repo_id);
        tokio::fs::create_dir_all(&repo_dir).await?;
        for i in 0..=0xFFu16 {
            tokio::fs::create_dir_all(repo_dir.join(format!("{i:02x}"))).await?;
        }
        // The marker is what lets an operator (or the GC orphan sweep) map a
        // hashed directory name back to a repository id.
        tokio::fs::write(repo_dir.join(REPO_MARKER_FILE), repo_id.as_bytes()).await?;

        self.prepared_repos
            .lock()
            .unwrap()
            .insert(repo_id.to_string());
        Ok(())
    }

    // ── Atomic block publication ───────────────────────────────────────────

    /// Write `data` to a temp file next to its final path and flush it, so the
    /// bytes are durable before they can become visible under the block's id.
    ///
    /// Returns the temp path and the final path; the caller publishes the block
    /// and always removes the temp file.
    async fn stage_block(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> io::Result<(PathBuf, PathBuf)> {
        self.ensure_repo_dirs(repo_id).await?;
        let path = self.block_path(repo_id, block_id);
        let tmp = path.with_file_name(format!("{block_id}.{}.tmp", uuid::Uuid::new_v4()));
        if let Err(e) = tokio::fs::write(&tmp, data).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        // Flush the bytes before they become visible under the final id: a crash
        // must not leave a published-but-empty block.
        if let Ok(f) = tokio::fs::OpenOptions::new().write(true).open(&tmp).await {
            let _ = f.sync_all().await;
        }
        Ok((tmp, path))
    }

    /// Publish a staged block under its final path, reporting whether this call
    /// created it.
    ///
    /// `hard_link` is the publish that cannot overwrite: it fails when the block
    /// is already there, which keeps `was_new` race-free under concurrent
    /// writers of the same id. Filesystems without hard links fall back to an
    /// atomic `rename` (overwrite), where `was_new` comes from a prior existence
    /// check — the flag only feeds quota cleanup, which re-checks references
    /// before deleting.
    async fn publish_block(tmp: &Path, path: &Path) -> io::Result<bool> {
        match tokio::fs::hard_link(tmp, path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(_) => {
                let existed = tokio::fs::try_exists(path).await.unwrap_or(false);
                tokio::fs::rename(tmp, path).await?;
                Ok(!existed)
            }
        }
    }

    /// Store `data` under `block_id`, overwriting whatever is there when
    /// `exclusive` is false. Returns whether this call wrote the block.
    ///
    /// The bytes are staged and flushed first, so a write that is interrupted
    /// (or a process that is killed) leaves at most a `.tmp` file beside the
    /// block and never a short block at the final content-addressed path.
    async fn store_block(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
        exclusive: bool,
    ) -> io::Result<bool> {
        let (tmp, path) = self.stage_block(repo_id, block_id, data).await?;
        let result = if exclusive {
            Self::publish_block(&tmp, &path).await
        } else {
            tokio::fs::rename(&tmp, &path).await.map(|()| true)
        };
        let _ = tokio::fs::remove_file(&tmp).await;
        let was_new = result?;
        self.exists_cache_insert(repo_id, block_id, data.len() as u64);
        Ok(was_new)
    }

    /// An existing block whose stored length does not match the content being
    /// written cannot be the right bytes (the id is their sha1), so it is
    /// replaced. Log it: it means an interrupted write was left behind before
    /// blocks were published atomically.
    fn warn_stored_len_mismatch(repo_id: &str, block_id: &str, stored: u64, expected: u64) {
        tracing::warn!(
            "block {block_id} in repo {repo_id} stores {stored} bytes but its content has \
             {expected}; rewriting it"
        );
    }

    // ── Layout / migration helpers ─────────────────────────────────────────
    //
    // These operate on raw files and are used by the one-shot legacy-layout
    // migration. They live on the concrete backend (not the trait) because the
    // migration must move stored bytes verbatim, bypassing the at-rest
    // encryption decorator.

    /// Path of the layout marker in the block root.
    pub fn layout_marker_path(&self) -> PathBuf {
        self.base_dir.join(LAYOUT_MARKER_FILE)
    }

    /// Whether the block root is already on the per-repository layout.
    pub async fn layout_is_current(&self) -> bool {
        match tokio::fs::read_to_string(self.layout_marker_path()).await {
            Ok(v) => v.trim() == LAYOUT_VERSION,
            Err(_) => false,
        }
    }

    /// Record that the block root is on the per-repository layout.
    pub async fn write_layout_marker(&self) -> io::Result<()> {
        tokio::fs::write(self.layout_marker_path(), LAYOUT_VERSION.as_bytes()).await
    }

    /// Top-level directories of the legacy flat layout, i.e. every entry of the
    /// block root whose name is exactly two hex digits.
    ///
    /// The per-repository layout keeps its data under [`REPOS_DIR`] and its
    /// markers under dot-prefixed names, so a two-hex-digit directory can only
    /// be the legacy layout.
    pub async fn legacy_layout_dirs(&self) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.base_dir).await {
            Ok(e) => e,
            // No block root yet (fresh install): nothing legacy to migrate.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.len() == 2 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
                out.push(entry.path());
            }
        }
        Ok(out)
    }

    /// Whether the legacy flat layout is still present on disk.
    pub async fn legacy_layout_present(&self) -> io::Result<bool> {
        Ok(!self.legacy_layout_dirs().await?.is_empty())
    }

    /// Path a block would have had under the legacy flat layout.
    pub fn legacy_block_path(&self, block_id: &str) -> PathBuf {
        let prefix = block_id.get(..2).unwrap_or(block_id);
        self.base_dir.join(prefix).join(block_id)
    }

    /// Every `(block_id, byte_len)` present in the legacy flat layout.
    pub async fn legacy_blocks(&self) -> io::Result<Vec<(String, u64)>> {
        let mut out = Vec::new();
        for dir in self.legacy_layout_dirs().await? {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                if !entry.file_type().await?.is_file() {
                    continue;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
                    let len = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                    out.push((name.to_string(), len));
                }
            }
        }
        Ok(out)
    }

    /// Copy one block from the legacy flat layout into `repo_id`'s directory.
    ///
    /// Pure copy — no hard links, no symlinks — so it behaves identically on
    /// every supported platform and filesystem, and so purging the legacy tree
    /// afterwards cannot break the new one. The copy goes to a temporary file in
    /// the destination directory and is renamed into place, so an interrupted
    /// migration can never leave a half-written block behind.
    pub async fn migrate_legacy_block(
        &self,
        repo_id: &str,
        block_id: &str,
    ) -> io::Result<LegacyCopyOutcome> {
        if !Self::is_valid_block_id(block_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        let src = self.legacy_block_path(block_id);
        let src_len = match tokio::fs::metadata(&src).await {
            Ok(m) if m.is_file() => m.len(),
            _ => return Ok(LegacyCopyOutcome::MissingSource),
        };

        let dst = self.block_path(repo_id, block_id);
        if let Ok(m) = tokio::fs::metadata(&dst).await
            && m.is_file()
        {
            if m.len() == src_len {
                return Ok(LegacyCopyOutcome::AlreadyPresent);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "block {block_id} already exists with a different size \
                     (dest {}, source {src_len})",
                    m.len()
                ),
            ));
        }

        self.ensure_repo_dirs(repo_id).await?;
        let tmp = dst.with_file_name(format!("{block_id}.{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::copy(&src, &tmp).await?;
        {
            // Flush the copied bytes to stable storage before the rename, so a
            // crash can never expose a renamed-but-empty block.
            let f = tokio::fs::OpenOptions::new().write(true).open(&tmp).await?;
            let _ = f.sync_all().await;
        }
        if let Err(e) = tokio::fs::rename(&tmp, &dst).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        self.exists_cache_insert(repo_id, block_id, src_len);
        Ok(LegacyCopyOutcome::Copied)
    }

    /// Remove every top-level directory of the legacy flat layout.
    ///
    /// Idempotent: safe to re-run after an interrupted purge. Returns the number
    /// of directories removed.
    pub async fn purge_legacy_layout(&self) -> io::Result<u64> {
        let mut removed = 0u64;
        for dir in self.legacy_layout_dirs().await? {
            tokio::fs::remove_dir_all(&dir).await?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Remove any stray temporary files left by an interrupted migration or an
    /// interrupted block write. Called once at startup: blocks are published by
    /// an atomic link/rename, so a `.tmp` file is never a block, only leftover
    /// bytes.
    pub async fn purge_layout_temp_files(&self) -> io::Result<u64> {
        let mut removed = 0u64;
        let repos = self.base_dir.join(REPOS_DIR);
        let mut repo_entries = match tokio::fs::read_dir(&repos).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        while let Some(repo_entry) = repo_entries.next_entry().await? {
            if !repo_entry.file_type().await?.is_dir() {
                continue;
            }
            let mut prefixes = tokio::fs::read_dir(repo_entry.path()).await?;
            while let Some(prefix) = prefixes.next_entry().await? {
                if !prefix.file_type().await?.is_dir() {
                    continue;
                }
                let mut files = tokio::fs::read_dir(prefix.path()).await?;
                while let Some(file) = files.next_entry().await? {
                    let name = file.file_name();
                    let Some(name) = name.to_str() else { continue };
                    if name.ends_with(".tmp") {
                        tokio::fs::remove_file(file.path()).await?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }
}

#[async_trait]
impl BlockStorageBackend for BlockStorage {
    async fn has_block(&self, repo_id: &str, block_id: &str) -> bool {
        if !Self::is_valid_block_id(block_id) {
            return false;
        }
        if self.exists_cache_contains(repo_id, block_id) {
            return true;
        }
        let path = self.block_path(repo_id, block_id);
        // Record the size too: the write paths use it to tell a complete block
        // from a partial write left by an interrupted one.
        match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() => {
                self.exists_cache_insert(repo_id, block_id, meta.len());
                true
            }
            _ => false,
        }
    }

    async fn read_block(&self, repo_id: &str, block_id: &str) -> Result<Vec<u8>, io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        tokio::fs::read(self.block_path(repo_id, block_id)).await
    }

    /// Read at most `min(max, MAX_BLOCK_PREFIX_LEN)` leading bytes.
    ///
    /// A real partial read (open + bounded read) rather than [`Self::read_block`],
    /// so probing a large block's on-disk format header never pulls the whole
    /// block off disk: `get_block_map` and `range_stream`'s prefix skip probe
    /// every block of a file.
    async fn read_block_prefix(
        &self,
        repo_id: &str,
        block_id: &str,
        max: usize,
    ) -> io::Result<Vec<u8>> {
        if !Self::is_valid_block_id(block_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        let want = max.min(MAX_BLOCK_PREFIX_LEN);
        if want == 0 {
            return Ok(Vec::new());
        }
        use tokio::io::AsyncReadExt;
        let mut file = tokio::fs::File::open(self.block_path(repo_id, block_id)).await?;
        let mut buf = vec![0u8; want];
        let mut filled = 0usize;
        while filled < want {
            let n = file.read(&mut buf[filled..]).await?;
            if n == 0 {
                break; // short block: the file is smaller than `max`
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    }

    async fn write_block(&self, repo_id: &str, data: &[u8]) -> Result<String, io::Error> {
        let block_id = sha1_hex(data);
        self.write_block_with_id(repo_id, &block_id, data).await
    }

    async fn write_block_with_id(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, io::Error> {
        // The caller has already computed `block_id` as the SHA-1 of `data`
        // (and verified it), so write under it directly without re-hashing.
        //
        // Validate the shape anyway: `block_path` joins this id onto `base_dir`,
        // so an unvalidated `../../x` from a future caller would escape the
        // block directory. Every sibling method already fails closed here.
        if !Self::is_valid_block_id(block_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        let expected_len = data.len() as u64;

        // Content-addressed storage: identical content yields the same SHA-1,
        // so skip the write when the block is already stored at its full length.
        // Re-uploads and sync retries hit this path constantly.
        if self.exists_cache_len(repo_id, block_id) == Some(expected_len) {
            return Ok(block_id.to_string());
        }
        let path = self.block_path(repo_id, block_id);
        match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() && meta.len() == expected_len => {
                self.exists_cache_insert(repo_id, block_id, meta.len());
                return Ok(block_id.to_string());
            }
            Ok(meta) if meta.is_file() => {
                // A block stored at a different length cannot be the bytes this
                // id names: it is a partial write left by an interrupted upload
                // from before blocks were published atomically. Replace it
                // rather than treating it as a dedup hit, which would keep every
                // reader (and every file that references it) truncated.
                Self::warn_stored_len_mismatch(repo_id, block_id, meta.len(), expected_len);
                self.store_block(repo_id, block_id, data, false).await?;
                return Ok(block_id.to_string());
            }
            _ => {}
        }

        self.store_block(repo_id, block_id, data, true).await?;
        Ok(block_id.to_string())
    }

    async fn write_block_with_id_force(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, io::Error> {
        // Same as `write_block_with_id` but never short-circuits on an existing
        // block: the caller (lazy→encrypted conversion) needs to overwrite a
        // legacy plaintext block with its ciphertext under the same ID.
        if !Self::is_valid_block_id(block_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        self.store_block(repo_id, block_id, data, false).await?;
        Ok(block_id.to_string())
    }

    async fn write_block_with_id_tracked(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<(String, bool), io::Error> {
        // Race-free tracked write: the publish is exclusive, so when two
        // concurrent calls write the same block only one sees `was_new = true`
        // and the quota-cleanup path can safely delete only the blocks this
        // upload actually created. The bytes are staged and published
        // atomically, so an interrupted write leaves a `.tmp` file, never a
        // short block at the final path.
        if !Self::is_valid_block_id(block_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        let expected_len = data.len() as u64;
        if self.exists_cache_len(repo_id, block_id) == Some(expected_len) {
            return Ok((block_id.to_string(), false));
        }
        let path = self.block_path(repo_id, block_id);
        if let Ok(meta) = tokio::fs::metadata(&path).await
            && meta.is_file()
            && meta.len() != expected_len
        {
            Self::warn_stored_len_mismatch(repo_id, block_id, meta.len(), expected_len);
            self.store_block(repo_id, block_id, data, false).await?;
            return Ok((block_id.to_string(), true));
        }
        let was_new = self.store_block(repo_id, block_id, data, true).await?;
        Ok((block_id.to_string(), was_new))
    }

    async fn remove_block(&self, repo_id: &str, block_id: &str) -> Result<(), io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Ok(());
        }
        let path = self.block_path(repo_id, block_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        // Invalidate the cached presence regardless: even if the file was
        // already gone, a stale "exists" entry must not survive a remove.
        self.exists_cache_remove(repo_id, block_id);
        Ok(())
    }

    fn invalidate_exists_cache(&self) {
        self.exists_cache.lock().unwrap().clear();
    }

    async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        let path = self.block_path(repo_id, block_id);
        let size = tokio::fs::metadata(&path).await.map(|m| m.len() as i64)?;
        self.exists_cache_insert(repo_id, block_id, size.max(0) as u64);
        Ok(size)
    }

    async fn block_modified_secs(
        &self,
        repo_id: &str,
        block_id: &str,
    ) -> Result<Option<i64>, io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Ok(None);
        }
        let path = self.block_path(repo_id, block_id);
        match tokio::fs::metadata(&path).await {
            Ok(meta) => Ok(meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs() as i64)
            })),
            // A block that vanished between the enumeration and this stat is
            // simply gone: report "no age" rather than failing the whole pass.
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn list_blocks(&self, repo_id: &str) -> Result<Vec<String>, io::Error> {
        let mut blocks = Vec::new();
        let repo_dir = self.repo_dir(repo_id);
        let mut entries = match tokio::fs::read_dir(&repo_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(blocks),
            Err(e) => return Err(e),
        };

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let prefix = entry.file_name();
                let prefix_str = prefix.to_string_lossy();
                if prefix_str.len() == 2 {
                    let mut sub_entries = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(sub_entry) = sub_entries.next_entry().await? {
                        // Only include regular files matching 40-char hex IDs
                        if sub_entry.file_type().await?.is_file()
                            && let Some(name) = sub_entry.file_name().to_str()
                            && name.len() == 40
                            && name.bytes().all(|b| b.is_ascii_hexdigit())
                        {
                            blocks.push(name.to_string());
                        }
                    }
                }
            }
        }

        Ok(blocks)
    }

    /// Stream every block ID of one repository directly from the three-level
    /// directory layout without materialising the full list in memory.
    async fn for_each_block_in_repo(
        &self,
        repo_id: &str,
        mut f: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<(), io::Error> {
        let repo_dir = self.repo_dir(repo_id);
        let mut entries = match tokio::fs::read_dir(&repo_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let prefix = entry.file_name();
                let prefix_str = prefix.to_string_lossy();
                if prefix_str.len() == 2 {
                    let mut sub_entries = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(sub_entry) = sub_entries.next_entry().await? {
                        // Only visit regular files matching 40-char hex IDs
                        if sub_entry.file_type().await?.is_file() {
                            let name = sub_entry.file_name();
                            if let Some(name) = name.to_str()
                                && name.len() == 40
                                && name.bytes().all(|b| b.is_ascii_hexdigit())
                            {
                                f(name);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Stream every block id of one repository to a channel as the directory
    /// walk finds them, so a caller that awaits on each id never holds the
    /// whole list. Stops early when the receiver goes away.
    async fn stream_blocks_in_repo(
        &self,
        repo_id: &str,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<(), io::Error> {
        let repo_dir = self.repo_dir(repo_id);
        let mut entries = match tokio::fs::read_dir(&repo_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let prefix = entry.file_name();
                if prefix.to_string_lossy().len() != 2 {
                    continue;
                }
                let mut sub_entries = tokio::fs::read_dir(entry.path()).await?;
                while let Some(sub_entry) = sub_entries.next_entry().await? {
                    if !sub_entry.file_type().await?.is_file() {
                        continue;
                    }
                    let name = sub_entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    if name.len() != 40 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                        continue;
                    }
                    if tx.send(name.to_string()).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }

        Ok(())
    }

    async fn repo_dirs(&self) -> Result<Vec<(String, PathBuf)>, io::Error> {
        let mut out = Vec::new();
        let repos_root = self.base_dir.join(REPOS_DIR);
        let mut entries = match tokio::fs::read_dir(&repos_root).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let path = entry.path();
            // A directory without a readable marker cannot be attributed to a
            // repository (interrupted write, or a manual copy); report it under
            // its directory name so callers can still surface it.
            let marker = tokio::fs::read_to_string(path.join(REPO_MARKER_FILE))
                .await
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let Some(repo_id) = marker else { continue };
            out.push((repo_id, path));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed repository id for the tests: every operation is now scoped to a
    /// repository, and using two of them is how the isolation is asserted.
    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";
    const OTHER: &str = "11111111-2222-3333-4444-555555555555";

    /// Create a unique temp layout `{tmp}/nf-blockstore-{uuid}/data/blocks` and
    /// return `(root, store)`. The `data` level makes path-traversal ids resolve
    /// through real directories (the kernel rejects `..` through non-existent
    /// dirs).
    fn temp_storage() -> (PathBuf, BlockStorage) {
        let root = std::env::temp_dir().join(format!("nf-blockstore-{}", uuid::Uuid::new_v4()));
        let blocks = root.join("data").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        (root, BlockStorage::new(blocks))
    }

    #[tokio::test]
    async fn valid_block_roundtrip_works() {
        let (_root, store) = temp_storage();
        let id = store.write_block(REPO, b"hello world").await.unwrap();
        assert_eq!(id.len(), 40);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(store.has_block(REPO, &id).await);
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), b"hello world");
        assert!(store.block_size(REPO, &id).await.unwrap() > 0);
        store.remove_block(REPO, &id).await.unwrap();
        assert!(!store.has_block(REPO, &id).await);
    }

    /// The whole point of the per-repository layout: a block written for one
    /// repository is invisible and unreadable through another one.
    #[tokio::test]
    async fn blocks_are_isolated_per_repository() {
        let (_root, store) = temp_storage();
        let id = store.write_block(REPO, b"secret").await.unwrap();

        assert!(store.has_block(REPO, &id).await);
        assert!(
            !store.has_block(OTHER, &id).await,
            "another repository must not see the block"
        );
        assert!(
            store.read_block(OTHER, &id).await.is_err(),
            "another repository must not read the block"
        );
        assert!(store.block_size(OTHER, &id).await.is_err());
        assert!(store.list_blocks(OTHER).await.unwrap().is_empty());

        // Removing it through the wrong repository must not delete the block.
        store.remove_block(OTHER, &id).await.unwrap();
        assert!(store.has_block(REPO, &id).await);
    }

    /// The streaming visitor must yield exactly the same ids as `list_blocks`.
    #[tokio::test]
    async fn for_each_block_matches_list_blocks() {
        let (_root, store) = temp_storage();
        for i in 0..3 {
            let _ = store
                .write_block(REPO, format!("content {i}").as_bytes())
                .await
                .unwrap();
        }

        let mut listed = store.list_blocks(REPO).await.unwrap();
        let visited = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = visited.clone();
        store
            .for_each_block_in_repo(
                REPO,
                Box::new(move |id| sink.lock().unwrap().push(id.to_string())),
            )
            .await
            .unwrap();

        listed.sort();
        let mut visited = visited.lock().unwrap().clone();
        visited.sort();
        assert_eq!(listed, visited);
    }

    #[tokio::test]
    async fn write_block_dedup_returns_same_id() {
        let (_root, store) = temp_storage();
        let id1 = store.write_block(REPO, b"same content").await.unwrap();
        let id2 = store.write_block(REPO, b"same content").await.unwrap();
        assert_eq!(id1, id2);
        assert!(store.has_block(REPO, &id1).await);
        // Dedup is per repository: the same bytes under another repository get
        // their own copy.
        let id3 = store.write_block(OTHER, b"same content").await.unwrap();
        assert_eq!(id1, id3);
        assert!(store.has_block(OTHER, &id3).await);
        assert_ne!(store.block_path(REPO, &id1), store.block_path(OTHER, &id3));
    }

    #[tokio::test]
    async fn path_traversal_block_ids_are_rejected() {
        let (root, store) = temp_storage();
        // A file living *outside* the block tree (sibling of `blocks/`).
        let secret = root.join("secret.txt");
        tokio::fs::write(&secret, b"secret").await.unwrap();

        let traversal = "../secret.txt";

        // Must not be readable through the block store…
        assert!(store.read_block(REPO, traversal).await.is_err());
        assert!(!store.has_block(REPO, traversal).await);
        assert!(store.block_size(REPO, traversal).await.is_err());
        // …and remove_block must not delete it.
        store.remove_block(REPO, traversal).await.unwrap();
        assert_eq!(tokio::fs::read(&secret).await.unwrap(), b"secret");
    }

    /// `write_block_with_id` must validate the id it is handed: `block_path`
    /// joins it onto the block directory, so an unvalidated value from a future
    /// caller would escape the tree on write.
    #[tokio::test]
    async fn write_block_with_id_rejects_malformed_ids() {
        let (root, store) = temp_storage();
        let secret = root.join("secret.txt");
        tokio::fs::write(&secret, b"secret").await.unwrap();

        for bad in ["../secret.txt", "nothex", "", "a", "..", "../../evil"] {
            assert!(
                store
                    .write_block_with_id(REPO, bad, b"payload")
                    .await
                    .is_err(),
                "write_block_with_id({bad:?}) must be rejected"
            );
        }

        // Nothing leaked into or out of the block tree.
        assert_eq!(tokio::fs::read(&secret).await.unwrap(), b"secret");
        assert!(
            store.list_blocks(REPO).await.unwrap().is_empty(),
            "no block file may be created for a rejected id"
        );
    }

    #[tokio::test]
    async fn malformed_or_short_block_ids_do_not_panic() {
        let (_root, store) = temp_storage();
        let bad_ids = [
            "",                                         // too short to even take a prefix
            "a",                                        // too short
            "..",                                       // directory traversal
            "nothex",                                   // valid length, not hex
            "abcdef1234567890abcdef1234567890abcdef12", // 40 hex, valid shape but absent
        ];
        for bad in bad_ids {
            assert!(
                !store.has_block(REPO, bad).await,
                "has_block({bad:?}) should be false"
            );
            assert!(
                store.read_block(REPO, bad).await.is_err(),
                "read_block({bad:?}) should be Err"
            );
            assert!(
                store.block_size(REPO, bad).await.is_err(),
                "block_size({bad:?}) should be Err"
            );
        }
    }

    #[tokio::test]
    async fn read_block_rejects_absolute_escape() {
        let (_root, store) = temp_storage();
        // Worst case: a traversal id pointing at a real system file.
        assert!(
            store
                .read_block(REPO, "../../../../etc/passwd")
                .await
                .is_err()
        );
        assert!(!store.has_block(REPO, "../../../../etc/passwd").await);
    }

    #[test]
    fn block_path_prefix_uses_first_two_chars() {
        let store = BlockStorage::new(PathBuf::from("/tmp"));
        let p = store.block_path("11111111-2222-3333-4444-555555555555", "abcdef");
        // {base}/repos/{sha1(repo)}/ab/abcdef
        let expected = Path::new("/tmp")
            .join(REPOS_DIR)
            .join(sha1_hex(b"11111111-2222-3333-4444-555555555555"))
            .join("ab")
            .join("abcdef");
        assert_eq!(p, expected);
    }

    /// `remove_block` must invalidate the cached presence entry, not just delete
    /// the file — otherwise a subsequent `has_block` returns a stale "exists".
    #[tokio::test]
    async fn exists_cache_clear_on_remove() {
        let (_root, store) = temp_storage();
        let id = store.write_block(REPO, b"cache-me").await.unwrap();
        assert!(store.has_block(REPO, &id).await); // populates the existence cache
        store.remove_block(REPO, &id).await.unwrap();
        assert!(
            !store.has_block(REPO, &id).await,
            "cached presence must be dropped"
        );
    }

    /// `invalidate_exists_cache` drops every cached presence so the next check
    /// re-stats disk instead of trusting a stale entry.
    #[tokio::test]
    async fn invalidate_exists_cache_forces_recheck() {
        let (_root, store) = temp_storage();
        let id = store.write_block(REPO, b"cache-me").await.unwrap();
        assert!(store.has_block(REPO, &id).await); // populates the existence cache
        store.invalidate_exists_cache();
        store.remove_block(REPO, &id).await.unwrap();
        assert!(
            !store.has_block(REPO, &id).await,
            "recheck after invalidation"
        );
    }

    // ── Layout migration ────────────────────────────────────────────────────

    /// Build a legacy flat layout holding `blocks`, then return the store.
    async fn legacy_store(blocks: &[(&str, &[u8])]) -> (PathBuf, BlockStorage) {
        let (root, store) = temp_storage();
        for (id, data) in blocks {
            let dir = store.base_dir().join(&id[..2]);
            tokio::fs::create_dir_all(&dir).await.unwrap();
            tokio::fs::write(dir.join(id), data).await.unwrap();
        }
        (root, store)
    }

    #[tokio::test]
    async fn legacy_layout_is_detected_and_purged() {
        let content = b"legacy content";
        let id = sha1_hex(content);
        let (_root, store) = legacy_store(&[(&id, content)]).await;

        assert!(store.legacy_layout_present().await.unwrap());
        let listed = store.legacy_blocks().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, id);

        assert_eq!(
            store.migrate_legacy_block(REPO, &id).await.unwrap(),
            LegacyCopyOutcome::Copied
        );
        assert!(store.has_block(REPO, &id).await);
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), content);

        // Re-running the migration is a no-op, not an error.
        assert_eq!(
            store.migrate_legacy_block(REPO, &id).await.unwrap(),
            LegacyCopyOutcome::AlreadyPresent
        );

        assert_eq!(store.purge_legacy_layout().await.unwrap(), 1);
        assert!(!store.legacy_layout_present().await.unwrap());
        // The migrated block survived the purge.
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), content);
    }

    #[tokio::test]
    async fn migrate_reports_blocks_missing_from_the_legacy_store() {
        let (_root, store) = legacy_store(&[]).await;
        let id = sha1_hex(b"never stored");
        assert_eq!(
            store.migrate_legacy_block(REPO, &id).await.unwrap(),
            LegacyCopyOutcome::MissingSource
        );
        assert!(!store.has_block(REPO, &id).await);
    }

    #[tokio::test]
    async fn repo_dirs_reports_the_marker_and_maps_it_back() {
        let (_root, store) = temp_storage();
        let id = store.write_block(REPO, b"x").await.unwrap();
        assert!(store.has_block(REPO, &id).await);

        let dirs = store.repo_dirs().await.unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].0, REPO);
        assert_eq!(dirs[0].1, store.repo_dir(REPO));
    }

    #[tokio::test]
    async fn layout_marker_roundtrip() {
        let (_root, store) = temp_storage();
        assert!(!store.layout_is_current().await);
        store.write_layout_marker().await.unwrap();
        assert!(store.layout_is_current().await);
    }

    #[tokio::test]
    async fn purge_layout_temp_files_removes_partial_copies() {
        let (_root, store) = temp_storage();
        let id = store.write_block(REPO, b"x").await.unwrap();
        let stray = store
            .block_path(REPO, &id)
            .with_file_name(format!("{id}.deadbeef.tmp"));
        tokio::fs::write(&stray, b"partial").await.unwrap();
        assert_eq!(store.purge_layout_temp_files().await.unwrap(), 1);
        assert!(!stray.exists());
        // The real block is untouched.
        assert!(store.has_block(REPO, &id).await);
    }

    /// A block stored at a shorter length than its id names is what an
    /// interrupted write left behind before blocks were published atomically.
    /// A later verified write of the same block must replace it rather than
    /// taking the dedup shortcut, which would keep every reader truncated.
    #[tokio::test]
    async fn write_block_repairs_a_short_stored_block() {
        let (_root, store) = temp_storage();
        let data = b"the complete block contents";
        let id = store.write_block(REPO, data).await.unwrap();

        // Keep the id, shorten the file: a partial write under a full id.
        std::fs::write(store.block_path(REPO, &id), &data[..4]).unwrap();
        store.invalidate_exists_cache();
        assert_eq!(store.block_size(REPO, &id).await.unwrap(), 4);

        assert_eq!(store.write_block_with_id(REPO, &id, data).await.unwrap(), id);
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        assert_eq!(store.block_size(REPO, &id).await.unwrap(), data.len() as i64);
    }

    /// The tracked (quota-accounted) write repairs the same way and reports
    /// that it wrote the block, so cleanup may treat it as this upload's own.
    #[tokio::test]
    async fn tracked_write_repairs_a_short_stored_block() {
        let (_root, store) = temp_storage();
        let data = b"tracked complete block";
        let id = store.write_block(REPO, data).await.unwrap();
        std::fs::write(store.block_path(REPO, &id), &data[..3]).unwrap();
        store.invalidate_exists_cache();

        let (returned, was_new) = store
            .write_block_with_id_tracked(REPO, &id, data)
            .await
            .unwrap();
        assert_eq!(returned, id);
        assert!(was_new, "replacing a partial block counts as writing it");
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
    }

    /// An intact block is still a dedup hit: no rewrite, `was_new = false`.
    #[tokio::test]
    async fn tracked_write_reports_an_intact_dedup_hit_as_not_new() {
        let (_root, store) = temp_storage();
        let data = b"dedup me";
        let id = store.write_block(REPO, data).await.unwrap();
        store.invalidate_exists_cache();

        let (returned, was_new) = store
            .write_block_with_id_tracked(REPO, &id, data)
            .await
            .unwrap();
        assert_eq!(returned, id);
        assert!(!was_new);
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
    }

    /// Neither the fresh-write nor the repair path may leave its staged `.tmp`
    /// file behind: only an interrupted process leaves one, and startup sweeps
    /// those.
    #[tokio::test]
    async fn block_writes_leave_no_temp_files() {
        let (root, store) = temp_storage();
        let data = b"staged write";
        let id = store.write_block(REPO, data).await.unwrap();
        // Repair path as well.
        std::fs::write(store.block_path(REPO, &id), b"short").unwrap();
        store.invalidate_exists_cache();
        store.write_block_with_id(REPO, &id, data).await.unwrap();

        let mut leftovers = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    stack.push(entry.path());
                } else if entry.file_name().to_string_lossy().ends_with(".tmp") {
                    leftovers.push(entry.path());
                }
            }
        }
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
