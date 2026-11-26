//! Registry of unconsumed zip-download tokens.
//!
//! A minted token carries the full entry list until its GET consumes it, so the
//! registry is bounded by both a token count and a byte budget. It is process-wide
//! state, which is why it lives beside the upload services rather than in the
//! handler that mints the tokens.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::fs::zip::ZipFileEntry;
/// Task info stored per zip-token.
pub(crate) struct ZipTaskInfo {
    pub(crate) repo_id: String,
    /// Who requested the archive; re-checked when the token is consumed so a user
    /// whose access was revoked (or account deactivated) within the one-hour TTL
    /// cannot still download it.
    pub(crate) user_id: i32,
    pub(crate) files: Vec<ZipFileEntry>,
    pub(crate) zip_name: String,
    pub(crate) created_at: i64,
}

/// Unconsumed zip-download tokens, bounded by **both** a count and a byte budget.
///
/// The byte budget decides memory use: a token carries the full entry list, and
/// `block_ids` is a per-file vector of 40-char ids, so a count cap alone would let
/// `max_zip_entries` entries times concurrently-held tokens reach gigabytes.
#[derive(Default)]
struct ZipTaskRegistry {
    tasks: HashMap<String, ZipTaskInfo>,
    /// Sum of [`ZipTaskInfo::entry_bytes`] over `tasks`, kept in step with every
    /// insert and removal so the budget check is O(1).
    bytes: usize,
}

impl ZipTaskInfo {
    /// Rough heap footprint of one registry entry, for the byte budget only.
    /// Deliberately an over-estimate: counts the struct, the map key and every
    /// owned string (including each block id).
    fn entry_bytes(token: &str, info: &Self) -> usize {
        let files: usize = info
            .files
            .iter()
            .map(|f| {
                std::mem::size_of::<ZipFileEntry>()
                    + f.path_in_zip.len()
                    + f.block_ids
                        .iter()
                        .map(|b| b.len() + std::mem::size_of::<String>())
                        .sum::<usize>()
            })
            .sum();
        std::mem::size_of::<Self>()
            + token.len()
            + std::mem::size_of::<String>()
            + info.repo_id.len()
            + info.zip_name.len()
            + files
    }
}

impl ZipTaskRegistry {
    /// Drop tokens older than [`ZIP_TASK_TTL_SECS`] and return how many went.
    fn sweep(&mut self, now: i64) -> usize {
        let mut dropped = 0usize;
        let mut freed = 0usize;
        self.tasks.retain(|token, task| {
            let keep = now - task.created_at < ZIP_TASK_TTL_SECS;
            if !keep {
                dropped += 1;
                freed += ZipTaskInfo::entry_bytes(token, task);
            }
            keep
        });
        self.bytes = self.bytes.saturating_sub(freed);
        dropped
    }

    /// Insert a token, evicting oldest-first until both budgets hold. Returns
    /// `false` when the entry alone exceeds `max_bytes` (never satisfiable).
    fn insert(
        &mut self,
        token: String,
        info: ZipTaskInfo,
        max_count: usize,
        max_bytes: usize,
    ) -> bool {
        let bytes = ZipTaskInfo::entry_bytes(&token, &info);
        if max_bytes > 0 && bytes > max_bytes {
            return false;
        }
        loop {
            let over_count = max_count > 0 && self.tasks.len() >= max_count;
            let over_bytes = max_bytes > 0 && self.bytes + bytes > max_bytes;
            if !over_count && !over_bytes {
                break;
            }
            let Some(oldest) = self
                .tasks
                .iter()
                .min_by_key(|(_, task)| task.created_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            if let Some(removed) = self.tasks.remove(&oldest) {
                self.bytes = self
                    .bytes
                    .saturating_sub(ZipTaskInfo::entry_bytes(&oldest, &removed));
            }
        }
        self.bytes += bytes;
        self.tasks.insert(token, info);
        true
    }

    /// Consume a token, returning its bytes to the budget.
    fn take(&mut self, token: &str) -> Option<ZipTaskInfo> {
        let removed = self.tasks.remove(token)?;
        self.bytes = self
            .bytes
            .saturating_sub(ZipTaskInfo::entry_bytes(token, &removed));
        Some(removed)
    }
}

static ZIP_TASKS: OnceLock<Mutex<ZipTaskRegistry>> = OnceLock::new();

/// TTL for an unconsumed zip task, and a hard cap on how many may be held at
/// once; the byte budget in [`ZipTaskRegistry`] is the binding limit in practice.
const ZIP_TASK_TTL_SECS: i64 = 3600;
pub(crate) const MAX_ZIP_TASKS: usize = 1000;

fn zip_tasks() -> &'static Mutex<ZipTaskRegistry> {
    ZIP_TASKS.get_or_init(|| Mutex::new(ZipTaskRegistry::default()))
}

/// Drop expired zip tokens, reporting how many were dropped so the cleanup job
/// can say what it did; called on task creation and periodically by the scheduler.
pub(crate) fn cleanup_expired(now: i64) -> usize {
    match zip_tasks().lock() {
        Ok(mut registry) => registry.sweep(now),
        // A poisoned lock means a previous pass panicked; the registry is still
        // readable, and the next pass will try again.
        Err(poisoned) => poisoned.into_inner().sweep(now),
    }
}

/// Store one minted token, enforcing the count and byte budgets; `false` when the
/// entry alone exceeds the byte budget.
pub(crate) fn mint(
    token: String,
    info: ZipTaskInfo,
    max_count: usize,
    max_bytes: usize,
) -> Result<bool, String> {
    let mut registry = zip_tasks()
        .lock()
        .map_err(|_| "zip task registry poisoned".to_string())?;
    Ok(registry.insert(token, info, max_count, max_bytes))
}

/// Consume one token, returning its entry and freeing its budget.
pub(crate) fn take(token: &str) -> Result<Option<ZipTaskInfo>, String> {
    let mut registry = zip_tasks()
        .lock()
        .map_err(|_| "zip task registry poisoned".to_string())?;
    Ok(registry.take(token))
}
#[cfg(test)]
mod registry_tests {
    use super::*;

    fn entry(path: &str, blocks: usize) -> ZipFileEntry {
        ZipFileEntry {
            path_in_zip: path.to_string(),
            block_ids: (0..blocks).map(|i| format!("{i:040x}")).collect(),
            size: 1,
        }
    }

    fn info(files: Vec<ZipFileEntry>, created_at: i64) -> ZipTaskInfo {
        ZipTaskInfo {
            repo_id: "repo".to_string(),
            user_id: 1,
            files,
            zip_name: "z".to_string(),
            created_at,
        }
    }

    fn bytes_of(token: &str, files: Vec<ZipFileEntry>) -> usize {
        ZipTaskInfo::entry_bytes(token, &info(files, 0))
    }

    /// The byte budget, not the token count, bounds memory: the oldest is evicted.
    #[test]
    fn byte_budget_evicts_the_oldest_tokens() {
        let mut registry = ZipTaskRegistry::default();
        let files = || vec![entry("a.txt", 40)];
        let one = bytes_of("t0", files());

        assert!(registry.insert("t0".into(), info(files(), 100), 0, one * 2));
        assert!(registry.insert("t1".into(), info(files(), 200), 0, one * 2));
        assert!(registry.insert("t2".into(), info(files(), 300), 0, one * 2));

        assert_eq!(registry.tasks.len(), 2, "the budget holds two tokens");
        assert!(!registry.tasks.contains_key("t0"), "the oldest is evicted");
        assert!(registry.tasks.contains_key("t1"));
        assert!(registry.tasks.contains_key("t2"));
        assert!(registry.bytes <= one * 2, "the tally stays within budget");
    }

    /// An archive whose listing alone exceeds the whole budget is refused rather
    /// than evicting everything else for something that still would not fit.
    #[test]
    fn an_oversized_token_is_refused_without_evicting() {
        let mut registry = ZipTaskRegistry::default();
        let one = bytes_of("small", vec![entry("a.txt", 4)]);
        assert!(registry.insert("small".into(), info(vec![entry("a.txt", 4)], 1), 0, one * 2));

        let big = vec![entry("big.bin", 4000)];
        assert!(
            !registry.insert("big".into(), info(big, 2), 0, one * 2),
            "an entry larger than the whole budget must be refused"
        );
        assert!(
            registry.tasks.contains_key("small"),
            "refusing must not evict the tokens already stored"
        );
    }

    /// The count cap evicts rather than failing; `take` frees the entry's bytes.
    #[test]
    fn count_cap_evicts_and_take_frees_bytes() {
        let mut registry = ZipTaskRegistry::default();
        assert!(registry.insert("t0".into(), info(vec![entry("a.txt", 4)], 1), 1, 0));
        assert_eq!(registry.tasks.len(), 1);
        let with_one = registry.bytes;

        assert!(registry.insert("t1".into(), info(vec![entry("a.txt", 4)], 2), 1, 0));
        assert_eq!(registry.tasks.len(), 1);
        assert!(
            !registry.tasks.contains_key("t0"),
            "the count cap evicts the oldest"
        );
        assert_eq!(registry.bytes, with_one, "the tally follows the eviction");

        assert!(registry.take("t1").is_some());
        assert_eq!(registry.bytes, 0, "consuming returns the bytes");
        assert!(registry.take("t1").is_none());
    }

    /// Expiry returns the bytes too, so abandoned tokens cannot pin the budget.
    #[test]
    fn sweep_drops_expired_tokens_and_their_bytes() {
        let mut registry = ZipTaskRegistry::default();
        let base = 1_000;
        let fresh_at = base + ZIP_TASK_TTL_SECS;
        // The token itself is part of an entry's footprint, so the two sizes
        // differ; compute each from the token it is actually stored under.
        let old_bytes = ZipTaskInfo::entry_bytes("old", &info(vec![entry("a.txt", 4)], base));
        let fresh_bytes =
            ZipTaskInfo::entry_bytes("fresh", &info(vec![entry("a.txt", 4)], fresh_at));

        registry.insert("old".into(), info(vec![entry("a.txt", 4)], base), 0, 0);
        registry.insert(
            "fresh".into(),
            info(vec![entry("a.txt", 4)], fresh_at),
            0,
            0,
        );
        assert_eq!(registry.bytes, old_bytes + fresh_bytes);

        registry.sweep(fresh_at + 1);

        assert_eq!(registry.tasks.len(), 1);
        assert!(
            !registry.tasks.contains_key("old"),
            "expired token is dropped"
        );
        assert!(registry.tasks.contains_key("fresh"));
        assert_eq!(
            registry.bytes, fresh_bytes,
            "the expired token's bytes are freed"
        );
    }
}
