use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;

/// In-memory cache mapping `(repo_id, root_fs_id, path)` → cached fs object.
///
/// After every write to a repo, call `clear_repo(repo_id)` to invalidate all
/// entries for that repo. Cache keys embed `root_fs_id` so stale entries from
/// a concurrent read-during-write are naturally invisible to subsequent lookups.
///
/// FIFO eviction when `max_entries` is exceeded — keeps memory bounded without
/// external dependencies.
pub struct PathCache {
    inner: RwLock<CacheData>,
    max_entries: usize,
}

const DEFAULT_MAX_ENTRIES: usize = 10_000;

impl Default for PathCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES)
    }
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct CacheKey {
    repo_id: String,
    root_fs_id: String,
    path: String,
}

#[derive(Clone, Debug)]
enum CacheEntry {
    File {
        fs_id: String,
        /// Serialized `FsFileData` JSON (avoids re-reading from the DB).
        json: String,
    },
    Dir {
        fs_id: String,
        /// Serialized `FsDirData` JSON (avoids re-reading from the DB).
        json: String,
    },
}

struct CacheData {
    entries: HashMap<CacheKey, CacheEntry>,
    order: VecDeque<CacheKey>,
}

impl PathCache {
    /// Create a cache with `max_entries` capacity (default: 10_000).
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: RwLock::new(CacheData {
                entries: HashMap::new(),
                order: VecDeque::new(),
            }),
            max_entries,
        }
    }

    /// Return `(obj_type, fs_id, json)` for a cached path.
    ///
    /// `obj_type` is `1` for files or `3` for directories, matching the
    /// [`SEAF_METADATA_TYPE_FILE`] / [`SEAF_METADATA_TYPE_DIR`] constants.
    pub fn get(&self, repo_id: &str, root_fs_id: &str, path: &str) -> Option<(i8, String, String)> {
        let guard = self.inner.read().ok()?;
        let key = CacheKey {
            repo_id: repo_id.to_owned(),
            root_fs_id: root_fs_id.to_owned(),
            path: path.to_owned(),
        };
        guard.entries.get(&key).map(|e| match e {
            CacheEntry::File { fs_id, json } => (1, fs_id.clone(), json.clone()),
            CacheEntry::Dir { fs_id, json } => (3, fs_id.clone(), json.clone()),
        })
    }

    /// Cache a file fs-object.
    ///
    /// The caller must pass the **serialized** JSON so we avoid a redundant
    /// `serde_json::to_string` call here.
    pub fn set_file(&self, repo_id: &str, root_fs_id: &str, path: &str, fs_id: &str, json: &str) {
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(_) => return,
        };
        let key = CacheKey {
            repo_id: repo_id.to_owned(),
            root_fs_id: root_fs_id.to_owned(),
            path: path.to_owned(),
        };

        // If the key already exists, remove it from order so we can re-push
        // to the back (treating it as "recently used").
        if guard.entries.contains_key(&key) {
            guard.order.retain(|k| k != &key);
        }

        // FIFO eviction if at capacity and this is a new unique entry.
        while guard.entries.len() >= self.max_entries && !guard.entries.contains_key(&key) {
            if let Some(oldest) = guard.order.pop_front() {
                guard.entries.remove(&oldest);
            } else {
                break;
            }
        }

        guard.order.push_back(key.clone());
        guard.entries.insert(
            key,
            CacheEntry::File {
                fs_id: fs_id.to_owned(),
                json: json.to_owned(),
            },
        );
    }

    /// Cache a directory fs-object.
    pub fn set_dir(&self, repo_id: &str, root_fs_id: &str, path: &str, fs_id: &str, json: &str) {
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(_) => return,
        };
        let key = CacheKey {
            repo_id: repo_id.to_owned(),
            root_fs_id: root_fs_id.to_owned(),
            path: path.to_owned(),
        };

        // If the key already exists, remove it from order so we can re-push
        // to the back (treating it as "recently used").
        if guard.entries.contains_key(&key) {
            guard.order.retain(|k| k != &key);
        }

        // FIFO eviction if at capacity and this is a new unique entry.
        while guard.entries.len() >= self.max_entries && !guard.entries.contains_key(&key) {
            if let Some(oldest) = guard.order.pop_front() {
                guard.entries.remove(&oldest);
            } else {
                break;
            }
        }

        guard.order.push_back(key.clone());
        guard.entries.insert(
            key,
            CacheEntry::Dir {
                fs_id: fs_id.to_owned(),
                json: json.to_owned(),
            },
        );
    }

    /// Remove all cached entries belonging to `repo_id`.
    ///
    /// Called after every write operation that modifies the repo's FS tree.
    pub fn clear_repo(&self, repo_id: &str) {
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(_) => return,
        };
        guard.order.retain(|k| k.repo_id != repo_id);
        guard.entries.retain(|k, _| k.repo_id != repo_id);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn make_key(repo_id: &str, root_fs_id: &str, path: &str) -> CacheKey {
        CacheKey {
            repo_id: repo_id.to_owned(),
            root_fs_id: root_fs_id.to_owned(),
            path: path.to_owned(),
        }
    }

    #[test]
    fn test_get_set_file() {
        let cache = PathCache::new(100);
        cache.set_file(
            "r1",
            "root1",
            "/a.txt",
            "f1",
            r#"{"block_ids":[],"size":0,"type":1,"version":1}"#,
        );
        let got = cache.get("r1", "root1", "/a.txt");
        assert!(got.is_some());
        let (ty, fs_id, _) = got.unwrap();
        assert_eq!(ty, 1);
        assert_eq!(fs_id, "f1");
    }

    #[test]
    fn test_get_set_dir() {
        let cache = PathCache::new(100);
        cache.set_dir(
            "r1",
            "root1",
            "/",
            "d_root",
            r#"{"dirents":[],"type":3,"version":1}"#,
        );
        let got = cache.get("r1", "root1", "/");
        assert!(got.is_some());
        let (ty, fs_id, _) = got.unwrap();
        assert_eq!(ty, 3);
        assert_eq!(fs_id, "d_root");
    }

    #[test]
    fn test_miss_wrong_repo() {
        let cache = PathCache::new(100);
        cache.set_file("r1", "root1", "/a.txt", "f1", "");
        assert!(cache.get("r2", "root1", "/a.txt").is_none());
    }

    #[test]
    fn test_miss_wrong_root() {
        let cache = PathCache::new(100);
        cache.set_file("r1", "root1", "/a.txt", "f1", "");
        assert!(cache.get("r1", "root2", "/a.txt").is_none());
    }

    #[test]
    fn test_miss_wrong_path() {
        let cache = PathCache::new(100);
        cache.set_file("r1", "root1", "/a.txt", "f1", "");
        assert!(cache.get("r1", "root1", "/b.txt").is_none());
    }

    #[test]
    fn test_clear_repo() {
        let cache = PathCache::new(100);
        cache.set_file("r1", "root1", "/a.txt", "f1", "");
        cache.set_file("r2", "root1", "/b.txt", "f2", "");
        cache.clear_repo("r1");
        assert!(cache.get("r1", "root1", "/a.txt").is_none());
        assert!(cache.get("r2", "root1", "/b.txt").is_some());
    }

    #[test]
    fn test_fifo_eviction() {
        let cache = PathCache::new(3);
        cache.set_file("r", "root", "/a", "fa", "");
        cache.set_file("r", "root", "/b", "fb", "");
        cache.set_file("r", "root", "/c", "fc", "");
        assert!(cache.get("r", "root", "/a").is_some()); // still in
        cache.set_file("r", "root", "/d", "fd", "");
        // "/a" should be evicted (first inserted)
        assert!(cache.get("r", "root", "/a").is_none());
        assert!(cache.get("r", "root", "/d").is_some());
    }

    #[test]
    fn test_update_same_key_does_not_evict() {
        let cache = PathCache::new(3);
        cache.set_file("r", "root", "/a", "f1", "");
        cache.set_file("r", "root", "/b", "f2", "");
        cache.set_file("r", "root", "/c", "f3", "");
        // Update "/a" — should NOT count as a new insertion for eviction.
        cache.set_file("r", "root", "/a", "f1_new", "");
        cache.set_file("r", "root", "/d", "f4", "");
        // "/a" should still be present (it was not the oldest — "/b" is).
        assert!(cache.get("r", "root", "/a").is_some());
        // "/b" should be evicted.
        assert!(cache.get("r", "root", "/b").is_none());
        assert!(cache.get("r", "root", "/d").is_some());
    }
}
