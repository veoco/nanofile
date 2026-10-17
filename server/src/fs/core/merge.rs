//! Server-side three-way merge of FS trees (`common/merge-new.c` upstream).
//!
//! `PUT /repo/{id}/commit/HEAD` merges instead of rejecting when the uploaded
//! commit's parent is no longer HEAD (`fast_forward_or_merge()`).
//!
//! Invariants: file contents never change — every output entry is a copy of a
//! head or remote dirent, mutated only by an `SFConflict` rename or a merged
//! sub-directory id. Entry lists are stored in **descending** name order
//! (`compare_dirents()` is `strcmp(b, a)`) and the walk depends on it; inputs
//! are sorted defensively because nanofile also writes directory objects where
//! nothing guarantees that order (sorting inputs, unlike rewriting an object,
//! does not change its id). `threeway_merge()` renames the remote entry in place
//! before `merge_directories()` sees it: D/F conflicts depend on that order.

use std::collections::HashMap;

use sea_orm::DatabaseConnection;

use crate::domain::fs::sort_dirents_desc;
use crate::fs::core::store::store_fs_dir_object;
use crate::fs::core::traversal::{TreeGuard, max_tree_depth};
use crate::fs::core::tree::read_fs_dir_data;
use crate::repository::Repositories;
use base::common::{DirEntryData, FsDirData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;

/// `S_IFMT` / `S_IFREG` / `S_IFDIR` type bits, like C's `S_ISREG()` / `S_ISDIR()`.
/// `base::common::S_IFREG` carries the permission bits (`0100644`), so it cannot
/// be the mask here.
const S_IFMT: i32 = 0o170000;
const S_IFREG_TYPE: i32 = 0o100000;
const S_IFDIR_TYPE: i32 = 0o040000;

fn is_reg(mode: i32) -> bool {
    mode & S_IFMT == S_IFREG_TYPE
}

fn is_dir(mode: i32) -> bool {
    mode & S_IFMT == S_IFDIR_TYPE
}

/// Result of merging one directory.
pub struct MergeOutcome {
    /// Root of the merged tree; `EMPTY_SHA1` when nothing is left, in which case
    /// no object is stored (like `seaf_dir_save()`).
    pub merged_root: String,
    /// Whether the merge renamed a conflicting entry (upstream's `opt.conflict`,
    /// the `conflict` flag of the merge commit).
    pub conflict: bool,
    /// Whether the merge kept an entry one side had *deleted or renamed* while the
    /// other side changed it (the mask 3 / 5 directory recursions also keep the old
    /// name). Upstream records no flag for these, but the kept copy sits at a path
    /// the deleting device never saved to, so a plain auto-merge with `conflict`
    /// unset can still contain such a path.
    pub kept_deleted: bool,
    /// Number of directories the merge walked (upstream's `opt.visit_dirs`).
    pub visit_dirs: usize,
}

/// Merge `remote_root` (the tree the client just uploaded, whose base commit is
/// `base_root`) with the repository's current `head_root`, three-way against
/// `base_root`. `base_root == EMPTY_SHA1` means "no common ancestor" and is treated
/// as an empty tree (upstream cannot reach that state; nanofile starts a library
/// with no HEAD at all). A missing or non-directory object is a hard error, like
/// upstream's -1: by the time the branch is advanced, every referenced object has
/// been received.
pub async fn merge_trees(
    repos: &Repositories,
    db: &DatabaseConnection,
    repo_id: &str,
    base_root: &str,
    head_root: &str,
    remote_root: &str,
    remote_creator_name: &str,
) -> Result<MergeOutcome, AppError> {
    let mut ctx = MergeCtx {
        repos,
        db,
        repo_id,
        remote_root,
        remote_creator_name,
        conflict: false,
        kept_deleted: false,
        visit_dirs: 0,
        guard: TreeGuard::new(),
        nicknames: HashMap::new(),
    };

    // Upstream requires all three roots to exist (`seaf_merge_trees()`).
    let base = read_dir(repos, repo_id, base_root).await?;
    let head = read_dir(repos, repo_id, head_root).await?;
    let remote = read_dir(repos, repo_id, remote_root).await?;

    let merged_root = Box::pin(merge_dir(
        &mut ctx,
        1,
        [Some(base), Some(head), Some(remote)],
        "",
    ))
    .await?;

    Ok(MergeOutcome {
        merged_root,
        conflict: ctx.conflict,
        kept_deleted: ctx.kept_deleted,
        visit_dirs: ctx.visit_dirs,
    })
}

async fn read_dir(repos: &Repositories, repo_id: &str, root: &str) -> Result<FsDirData, AppError> {
    read_fs_dir_data(repos, repo_id, root)
        .await
        .map_err(|e| AppError::Internal(format!("merge: cannot read tree {root}: {e}")))
}

struct MergeCtx<'a> {
    repos: &'a Repositories,
    db: &'a DatabaseConnection,
    repo_id: &'a str,
    /// Root of the remote commit, used to look up the last modifier of a
    /// conflicting file.
    remote_root: &'a str,
    remote_creator_name: &'a str,
    conflict: bool,
    kept_deleted: bool,
    visit_dirs: usize,
    guard: TreeGuard,
    /// modifier email → nickname, cached for the whole merge (upstream keeps the
    /// same cache in `opt->email_to_nickname`); `None` means no such user.
    nicknames: HashMap<String, Option<String>>,
}

/// The file-slot decision for one name group (upstream `threeway_merge()`, n = 3);
/// pure so the decision table can be unit-tested without a database.
#[derive(Debug, PartialEq, Eq)]
enum FileAction {
    /// Emit these entries verbatim.
    Emit(Vec<DirEntryData>),
    /// Conflict: keep `keep` (the head side) and rename `remote` with an `SFConflict` name.
    ConflictFile {
        keep: Option<DirEntryData>,
        remote: DirEntryData,
    },
    /// D/F conflict: rename the directory in slot `rename_slot`; `emit` is the head file.
    ConflictDir {
        emit: DirEntryData,
        rename_slot: usize,
    },
    /// Nothing to emit for the file slots of this name group.
    Nothing,
}

/// Upstream `merge_directories()`'s `dir_mask` switch, n = 3.
#[derive(Debug, PartialEq, Eq)]
enum DirAction {
    /// Emit the entry in this slot verbatim.
    Emit(usize),
    /// Emit nothing (the directory is deleted in the merged tree).
    Delete,
    /// Merge the sub-directories recursively, then emit slot `emit_slot` with the merged id.
    Recurse { emit_slot: usize },
}

/// `threeway_merge()` (`common/merge-new.c:174`), file slots only.
fn threeway_merge_files(slots: &[Option<DirEntryData>; 3]) -> FileAction {
    let f = |i: usize| slots[i].as_ref().filter(|d| is_reg(d.mode)).cloned();
    let (base, head, remote) = (f(0), f(1), f(2));
    // The D/F cases below test *presence* of the other slot, whatever its type.
    let (d0, d1, d2) = (slots[0].as_ref(), slots[1].as_ref(), slots[2].as_ref());

    if let (Some(head), Some(remote)) = (&head, &remote) {
        if head.id == remote.id {
            return FileAction::Emit(vec![head.clone()]);
        }
        if base.as_ref().is_some_and(|b| b.id == head.id) {
            return FileAction::Emit(vec![remote.clone()]);
        }
        if base.as_ref().is_some_and(|b| b.id == remote.id) {
            return FileAction::Emit(vec![head.clone()]);
        }
        // Both sides changed the same file: keep both, renaming remote's copy.
        return FileAction::ConflictFile {
            keep: Some(head.clone()),
            remote: remote.clone(),
        };
    }

    if let (Some(base), None, Some(remote)) = (&base, &head, &remote) {
        if base.id == remote.id {
            return FileAction::Nothing;
        }
        if d1.is_some() {
            // Head replaced the file with a directory while remote changed the
            // file: keep the directory at this name, rename remote's file.
            return FileAction::ConflictFile {
                keep: None,
                remote: remote.clone(),
            };
        }
        // Deleted in head and changed in remote: keep remote's version.
        return FileAction::Emit(vec![remote.clone()]);
    }

    if let (Some(base), Some(head), None) = (&base, &head, &remote) {
        if base.id == head.id {
            return FileAction::Nothing;
        }
        if d2.is_some() {
            // Remote replaced the file with a directory while head changed the
            // file: keep head's file, rename remote's directory.
            return FileAction::ConflictDir {
                emit: head.clone(),
                rename_slot: 2,
            };
        }
        return FileAction::Emit(vec![head.clone()]);
    }

    if let (None, None, Some(remote)) = (&base, &head, &remote) {
        if d1.is_none() {
            return FileAction::Emit(vec![remote.clone()]);
        }
        if d0.is_some_and(|d| d.id == d1.unwrap().id) {
            // The directory in head is unchanged; `merge_directories()` deletes
            // it and remote's file takes the name.
            return FileAction::Emit(vec![remote.clone()]);
        }
        // Head added a directory where remote added a file with the same name.
        return FileAction::ConflictFile {
            keep: None,
            remote: remote.clone(),
        };
    }

    if let (None, Some(head), None) = (&base, &head, &remote) {
        if d2.is_none() {
            return FileAction::Emit(vec![head.clone()]);
        }
        if d0.is_some_and(|d| d.id == d2.unwrap().id) {
            // The directory in remote is unchanged; `merge_directories()`
            // deletes it and head's file takes the name.
            return FileAction::Emit(vec![head.clone()]);
        }
        // Remote added a directory where head added a file with the same name.
        return FileAction::ConflictDir {
            emit: head.clone(),
            rename_slot: 2,
        };
    }

    // `base && !head && !remote`: deleted in both — nothing to emit. Every other
    // combination has a present slot and is handled above.
    FileAction::Nothing
}

/// `merge_directories()` (`common/merge-new.c:427`), the n = 3 `dir_mask`
/// switch.
fn merge_directories_action(slots: &[Option<DirEntryData>; 3]) -> Result<DirAction, AppError> {
    let mut mask = 0usize;
    for (i, slot) in slots.iter().enumerate() {
        if slot.as_ref().is_some_and(|d| is_dir(d.mode)) {
            mask |= 1 << i;
        }
    }
    let (d0, d1, d2) = (slots[0].as_ref(), slots[1].as_ref(), slots[2].as_ref());

    let action = match mask {
        // `g_return_val_if_reached(-1)`: the caller only calls this when at
        // least one slot is a directory.
        0 => return Err(AppError::Internal("merge: empty dir group".into())),
        1 => DirAction::Delete,
        2 => DirAction::Emit(1),
        // Base and head are directories.
        3 => {
            if d0.unwrap().id == d1.unwrap().id {
                // Deleted in remote.
                DirAction::Delete
            } else {
                DirAction::Recurse { emit_slot: 1 }
            }
        }
        4 => DirAction::Emit(2),
        // Base and remote are directories.
        5 => {
            if d0.unwrap().id == d2.unwrap().id {
                // Deleted in head.
                DirAction::Delete
            } else {
                DirAction::Recurse { emit_slot: 2 }
            }
        }
        // Head and remote are directories (6: base is not).
        6 | 7 => {
            if d1.unwrap().id == d2.unwrap().id {
                // The same in head and remote.
                DirAction::Emit(1)
            } else if d0.is_some_and(|d| d.id == d1.unwrap().id) {
                // Changed in remote, unchanged in head.
                DirAction::Emit(2)
            } else if d0.is_some_and(|d| d.id == d2.unwrap().id) {
                // Changed in head, unchanged in remote.
                DirAction::Emit(1)
            } else {
                DirAction::Recurse { emit_slot: 1 }
            }
        }
        _ => return Err(AppError::Internal("merge: invalid dir group".into())),
    };
    Ok(action)
}

/// `gen_conflict_path()` (`common/vc-common.c:584`). `time_buf` is passed in so the
/// formatting (leading-dot and extension-splitting rules) tests without a timezone.
pub(crate) fn gen_conflict_path(
    origin_path: &str,
    modifier: Option<&str>,
    time_buf: &str,
) -> String {
    let split = origin_path
        .rfind('.')
        .map(|dot| (&origin_path[..dot], &origin_path[dot + 1..]));
    match (modifier, split) {
        (Some(modifier), Some((stem, ext))) => {
            format!("{stem} (SFConflict {modifier} {time_buf}).{ext}")
        }
        (None, Some((stem, ext))) => format!("{stem} (SFConflict {time_buf}).{ext}"),
        (Some(modifier), None) => format!("{origin_path} (SFConflict {modifier} {time_buf})"),
        (None, None) => format!("{origin_path} (SFConflict {time_buf})"),
    }
}

/// `strftime("%Y-%m-%d-%H-%M-%S", localtime(mtime))`.
fn format_local_time(mtime: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(mtime, 0).single() {
        Some(t) => t.format("%Y-%m-%d-%H-%M-%S").to_string(),
        // `localtime()` would hand `strftime` a NULL `tm` and crash; use the raw timestamp.
        None => mtime.to_string(),
    }
}

impl MergeCtx<'_> {
    /// Nickname of a `modifier` email (seahub's `email2nickname()`, embedded in the
    /// conflict filename); falls back to the email itself, as upstream does when the
    /// seahub lookup fails.
    async fn nickname_for(&mut self, email: &str) -> Result<String, AppError> {
        if let Some(cached) = self.nicknames.get(email) {
            return Ok(cached.clone().unwrap_or_else(|| email.to_string()));
        }
        let nickname = self
            .repos
            .user
            .find_by_email(email)
            .await?
            .map(|u| u.nickname());
        self.nicknames.insert(email.to_string(), nickname.clone());
        Ok(nickname.unwrap_or_else(|| email.to_string()))
    }

    /// `get_file_modifier_mtime_v1()` (`common/vc-common.c:499`): the last modifier
    /// and mtime of `path` in the *remote* tree. `Ok(None)` means "walked, entry not
    /// there" (NULL modifier, mtime 0); `Err` makes the caller use the committer.
    async fn file_modifier_mtime(&self, path: &str) -> Result<Option<(String, i64)>, AppError> {
        let (parent, filename) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path),
        };

        let mut dir_id = self.remote_root.to_string();
        for segment in parent.split('/').filter(|s| !s.is_empty()) {
            let dir = read_dir(self.repos, self.repo_id, &dir_id).await?;
            let entry = dir
                .dirents
                .iter()
                .find(|d| d.name == segment && is_dir(d.mode))
                .ok_or_else(|| AppError::NotFound("merge: conflict path not found".into()))?;
            dir_id = entry.id.clone();
        }

        let dir = read_dir(self.repos, self.repo_id, &dir_id).await?;
        Ok(dir
            .dirents
            .iter()
            .find(|d| d.name == filename)
            .map(|d| (d.modifier.clone(), d.mtime)))
    }

    /// `merge_conflict_filename()` (`common/merge-new.c:40`).
    async fn conflict_file_name(&mut self, basedir: &str, name: &str) -> Result<String, AppError> {
        let path = format!("{basedir}{name}");
        let (modifier, mtime) = match self.file_modifier_mtime(&path).await {
            Ok(Some((modifier, mtime))) => (Some(modifier), mtime),
            Ok(None) => (None, 0),
            Err(_) => (
                Some(self.remote_creator_name.to_string()),
                chrono::Utc::now().timestamp(),
            ),
        };
        let nickname = match &modifier {
            Some(email) => Some(self.nickname_for(email).await?),
            None => None,
        };
        Ok(gen_conflict_path(
            name,
            nickname.as_deref(),
            &format_local_time(mtime),
        ))
    }

    /// `merge_conflict_dirname()`: always the remote commit's author and now.
    fn conflict_dir_name(&self, name: &str) -> String {
        gen_conflict_path(
            name,
            Some(self.remote_creator_name),
            &format_local_time(chrono::Utc::now().timestamp()),
        )
    }
}

/// `merge_trees_recursive()` (`common/merge-new.c:597`): merge the three
/// directories at one level and return the merged directory's id. `basedir` is the
/// result's path prefix here (no leading slash, `""` at the root, ending in `/`
/// below it), only used to build conflict names.
async fn merge_dir(
    ctx: &mut MergeCtx<'_>,
    depth: usize,
    mut trees: [Option<FsDirData>; 3],
    basedir: &str,
) -> Result<String, AppError> {
    if depth > max_tree_depth() {
        return Err(AppError::BadRequest(format!(
            "merge exceeded the maximum directory depth ({})",
            max_tree_depth()
        )));
    }
    ctx.guard.visit(1)?;
    ctx.visit_dirs += 1;

    // Upstream assumes descending order; see the module docs for why we sort.
    for tree in trees.iter_mut().flatten() {
        sort_dirents_desc(&mut tree.dirents);
    }

    let mut cursors = [0usize; 3];
    let mut merged: Vec<DirEntryData> = Vec::new();

    loop {
        // The "largest" name among the three cursors: the next one in descending order.
        let mut first_name: Option<String> = None;
        for (i, tree) in trees.iter().enumerate() {
            if let Some(dent) = tree.as_ref().and_then(|t| t.dirents.get(cursors[i]))
                && first_name.as_deref().is_none_or(|f| dent.name.as_str() > f)
            {
                first_name = Some(dent.name.clone());
            }
        }
        let Some(first_name) = first_name else {
            break;
        };

        let mut slots: [Option<DirEntryData>; 3] = [None, None, None];
        let mut n_files = 0;
        let mut n_dirs = 0;
        for (i, tree) in trees.iter().enumerate() {
            if let Some(dent) = tree.as_ref().and_then(|t| t.dirents.get(cursors[i]))
                && dent.name == first_name
            {
                if is_reg(dent.mode) {
                    n_files += 1;
                } else if is_dir(dent.mode) {
                    n_dirs += 1;
                }
                slots[i] = Some(dent.clone());
                cursors[i] += 1;
            }
        }

        // Merge the file slots of this level.
        if n_files > 0 {
            let action = threeway_merge_files(&slots);
            // The "deleted on one side, changed on the other" cases emit an entry
            // the deleting device never saved to; upstream records no flag for that,
            // so the caller needs to know (see `MergeOutcome::kept_deleted`).
            if matches!(action, FileAction::Emit(_))
                && slots[0].is_some()
                && (slots[1].is_none() != slots[2].is_none())
            {
                ctx.kept_deleted = true;
            }
            match action {
                FileAction::Nothing => {}
                FileAction::Emit(entries) => merged.extend(entries),
                FileAction::ConflictFile { keep, remote } => {
                    let name = ctx.conflict_file_name(basedir, &remote.name).await?;
                    if let Some(keep) = keep {
                        merged.push(keep);
                    }
                    let mut renamed = remote;
                    renamed.name = name;
                    merged.push(renamed);
                    ctx.conflict = true;
                }
                FileAction::ConflictDir { emit, rename_slot } => {
                    let dir = slots[rename_slot]
                        .as_ref()
                        .ok_or_else(|| AppError::Internal("merge: missing dir slot".into()))?;
                    let name = ctx.conflict_dir_name(&dir.name);
                    // Rename in place so the recursion below sees the new name.
                    slots[rename_slot].as_mut().unwrap().name = name;
                    merged.push(emit);
                    ctx.conflict = true;
                }
            }
        }

        // Recurse into the directory slots of this level.
        if n_dirs > 0 {
            match merge_directories_action(&slots)? {
                DirAction::Delete => {}
                DirAction::Emit(slot) => {
                    // `mask 2` / `mask 4` with a base entry on the other side: one
                    // device deleted the directory while this one kept it.
                    let other = if slot == 1 { 2 } else { 1 };
                    if slots[0].is_some() && slots[other].is_none() {
                        ctx.kept_deleted = true;
                    }
                    if let Some(dent) = &slots[slot] {
                        merged.push(dent.clone());
                    }
                }
                DirAction::Recurse { emit_slot } => {
                    // `mask 3` / `mask 5` recursion runs with the other side's slot
                    // empty, keeping the old name and the surviving side's changes.
                    let other = if emit_slot == 1 { 2 } else { 1 };
                    if slots[other].is_none() {
                        ctx.kept_deleted = true;
                    }
                    // Upstream takes the last directory slot's name as the child
                    // prefix; a slot renamed above is the one that ends up in the result.
                    let mut dirname = "";
                    for slot in slots.iter().flatten() {
                        if is_dir(slot.mode) {
                            dirname = &slot.name;
                        }
                    }
                    let new_basedir = format!("{basedir}{dirname}/");

                    let mut sub_trees: [Option<FsDirData>; 3] = [None, None, None];
                    for (i, slot) in slots.iter().enumerate() {
                        if let Some(dent) = slot
                            && is_dir(dent.mode)
                        {
                            sub_trees[i] = Some(read_dir(ctx.repos, ctx.repo_id, &dent.id).await?);
                        }
                    }

                    let merged_root =
                        Box::pin(merge_dir(ctx, depth + 1, sub_trees, &new_basedir)).await?;

                    let mut dent = slots[emit_slot]
                        .clone()
                        .ok_or_else(|| AppError::Internal("merge: missing dir slot".into()))?;
                    dent.id = merged_root;
                    merged.push(dent);
                }
            }
        }
    }

    sort_dirents_desc(&mut merged);

    let data = FsDirData {
        dirents: merged,
        obj_type: SEAF_METADATA_TYPE_DIR,
        version: 1,
    };
    store_fs_dir_object(ctx.db, ctx.repo_id, &data).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, name: &str) -> DirEntryData {
        DirEntryData {
            id: id.to_string(),
            mode: 0o100644,
            modifier: "mod@example.com".to_string(),
            mtime: 1_700_000_000,
            name: name.to_string(),
            size: 3,
        }
    }

    fn dir(id: &str, name: &str) -> DirEntryData {
        DirEntryData {
            id: id.to_string(),
            mode: 0o40000,
            modifier: String::new(),
            mtime: 1_700_000_000,
            name: name.to_string(),
            size: 0,
        }
    }

    fn group(
        a: Option<DirEntryData>,
        b: Option<DirEntryData>,
        c: Option<DirEntryData>,
    ) -> [Option<DirEntryData>; 3] {
        [a, b, c]
    }

    /// `gen_conflict_path()` byte for byte, including the empty-modifier case
    /// (upstream produces a double space there, because `""` is not NULL).
    #[test]
    fn conflict_path_matches_upstream() {
        let t = "2026-09-17-10-20-30";
        assert_eq!(
            gen_conflict_path("a.txt", Some("Alice"), t),
            "a (SFConflict Alice 2026-09-17-10-20-30).txt"
        );
        assert_eq!(
            gen_conflict_path("a.txt", None, t),
            "a (SFConflict 2026-09-17-10-20-30).txt"
        );
        assert_eq!(
            gen_conflict_path("report", Some("Alice"), t),
            "report (SFConflict Alice 2026-09-17-10-20-30)"
        );
        assert_eq!(
            gen_conflict_path("report", None, t),
            "report (SFConflict 2026-09-17-10-20-30)"
        );
        // Only the *last* dot splits the extension.
        assert_eq!(
            gen_conflict_path("a.b.c", Some("Alice"), t),
            "a.b (SFConflict Alice 2026-09-17-10-20-30).c"
        );
        // A leading dot is a dot like any other: the stem becomes empty.
        assert_eq!(
            gen_conflict_path(".bashrc", Some("Alice"), t),
            " (SFConflict Alice 2026-09-17-10-20-30).bashrc"
        );
        // Trailing dot: empty extension.
        assert_eq!(
            gen_conflict_path("a.", Some("Alice"), t),
            "a (SFConflict Alice 2026-09-17-10-20-30)."
        );
        // Empty nickname is not NULL.
        assert_eq!(
            gen_conflict_path("a.txt", Some(""), t),
            "a (SFConflict  2026-09-17-10-20-30).txt"
        );
        // Non-ASCII stem.
        assert_eq!(
            gen_conflict_path("报告.txt", Some("Alice"), t),
            "报告 (SFConflict Alice 2026-09-17-10-20-30).txt"
        );
    }

    #[test]
    fn dirents_sort_descending() {
        let mut v = vec![file("1", "a.txt"), dir("2", "b"), file("3", "C.txt")];
        sort_dirents_desc(&mut v);
        let names: Vec<&str> = v.iter().map(|d| d.name.as_str()).collect();
        // Byte order, descending: 'a' < 'b' < 'C'.
        assert_eq!(names, vec!["b", "a.txt", "C.txt"]);
    }

    #[test]
    fn files_match_emits_head() {
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("old", "f")),
                Some(file("new", "f")),
                Some(file("new", "f"))
            )),
            FileAction::Emit(vec![file("new", "f")])
        );
    }

    #[test]
    fn files_unchanged_in_head_changed_in_remote() {
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                Some(file("base", "f")),
                Some(file("remote", "f"))
            )),
            FileAction::Emit(vec![file("remote", "f")])
        );
    }

    #[test]
    fn files_unchanged_in_remote_changed_in_head() {
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                Some(file("head", "f")),
                Some(file("base", "f"))
            )),
            FileAction::Emit(vec![file("head", "f")])
        );
    }

    #[test]
    fn files_both_changed_conflicts() {
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                Some(file("head", "f")),
                Some(file("remote", "f"))
            )),
            FileAction::ConflictFile {
                keep: Some(file("head", "f")),
                remote: file("remote", "f")
            }
        );
    }

    #[test]
    fn files_deleted_in_head() {
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                None,
                Some(file("base", "f"))
            )),
            FileAction::Nothing
        );
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                None,
                Some(file("remote", "f"))
            )),
            FileAction::Emit(vec![file("remote", "f")])
        );
        // Head turned the name into a directory -> D/F conflict, remote renamed.
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                Some(dir("headdir", "f")),
                Some(file("remote", "f"))
            )),
            FileAction::ConflictFile {
                keep: None,
                remote: file("remote", "f")
            }
        );
    }

    #[test]
    fn files_deleted_in_remote() {
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                Some(file("base", "f")),
                None
            )),
            FileAction::Nothing
        );
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                Some(file("head", "f")),
                None
            )),
            FileAction::Emit(vec![file("head", "f")])
        );
        // Remote made the name a directory -> head's file kept, remote's dir renamed.
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                Some(file("head", "f")),
                Some(dir("remotedir", "f"))
            )),
            FileAction::ConflictDir {
                emit: file("head", "f"),
                rename_slot: 2
            }
        );
    }

    #[test]
    fn files_added_on_one_side() {
        assert_eq!(
            threeway_merge_files(&group(None, None, Some(file("remote", "f")))),
            FileAction::Emit(vec![file("remote", "f")])
        );
        // Same, but base is a directory head left unchanged, so the directory is
        // deleted and remote's file keeps the name.
        assert_eq!(
            threeway_merge_files(&group(
                Some(dir("samedir", "f")),
                Some(dir("samedir", "f")),
                Some(file("remote", "f"))
            )),
            FileAction::Emit(vec![file("remote", "f")])
        );
        assert_eq!(
            threeway_merge_files(&group(None, Some(file("head", "f")), None)),
            FileAction::Emit(vec![file("head", "f")])
        );
        assert_eq!(
            threeway_merge_files(&group(
                Some(dir("samedir", "f")),
                Some(file("head", "f")),
                Some(dir("samedir", "f"))
            )),
            FileAction::Emit(vec![file("head", "f")])
        );
        // Both sides added something with the same name, head a directory and
        // remote a file (or vice versa) -> D/F conflict.
        assert_eq!(
            threeway_merge_files(&group(
                None,
                Some(dir("headdir", "f")),
                Some(file("remote", "f"))
            )),
            FileAction::ConflictFile {
                keep: None,
                remote: file("remote", "f")
            }
        );
        assert_eq!(
            threeway_merge_files(&group(
                None,
                Some(file("head", "f")),
                Some(dir("remotedir", "f"))
            )),
            FileAction::ConflictDir {
                emit: file("head", "f"),
                rename_slot: 2
            }
        );
        // A base directory that changed in head, with remote adding a file:
        // the directories merge recursively and remote's file is renamed.
        assert_eq!(
            threeway_merge_files(&group(
                Some(dir("basedir", "f")),
                Some(dir("headdir", "f")),
                Some(file("remote", "f"))
            )),
            FileAction::ConflictFile {
                keep: None,
                remote: file("remote", "f")
            }
        );
        // ... and the mirror, where remote's directory changed and head added a
        // file with that name.
        assert_eq!(
            threeway_merge_files(&group(
                Some(dir("basedir", "f")),
                Some(file("head", "f")),
                Some(dir("remotedir", "f"))
            )),
            FileAction::ConflictDir {
                emit: file("head", "f"),
                rename_slot: 2
            }
        );
    }

    #[test]
    fn files_deleted_on_both_sides() {
        assert_eq!(
            threeway_merge_files(&group(Some(file("base", "f")), None, None)),
            FileAction::Nothing
        );
    }

    #[test]
    fn dirs_mask_table() {
        // mask 1: only base is a dir -> nothing.
        assert_eq!(
            merge_directories_action(&group(Some(dir("d", "n")), None, None)).unwrap(),
            DirAction::Delete
        );
        // mask 2 / 4: only head / only remote.
        assert_eq!(
            merge_directories_action(&group(None, Some(dir("hd", "n")), None)).unwrap(),
            DirAction::Emit(1)
        );
        assert_eq!(
            merge_directories_action(&group(None, None, Some(dir("rd", "n")))).unwrap(),
            DirAction::Emit(2)
        );
        // mask 3: base + head.
        assert_eq!(
            merge_directories_action(&group(Some(dir("d", "n")), Some(dir("d", "n")), None))
                .unwrap(),
            DirAction::Delete
        );
        assert_eq!(
            merge_directories_action(&group(Some(dir("d", "n")), Some(dir("h", "n")), None))
                .unwrap(),
            DirAction::Recurse { emit_slot: 1 }
        );
        // mask 5: base + remote.
        assert_eq!(
            merge_directories_action(&group(Some(dir("d", "n")), None, Some(dir("d", "n"))))
                .unwrap(),
            DirAction::Delete
        );
        assert_eq!(
            merge_directories_action(&group(Some(dir("d", "n")), None, Some(dir("r", "n"))))
                .unwrap(),
            DirAction::Recurse { emit_slot: 2 }
        );
        // mask 6/7: head + remote.
        assert_eq!(
            merge_directories_action(&group(None, Some(dir("x", "n")), Some(dir("x", "n"))))
                .unwrap(),
            DirAction::Emit(1)
        );
        assert_eq!(
            merge_directories_action(&group(
                Some(dir("d", "n")),
                Some(dir("d", "n")),
                Some(dir("r", "n"))
            ))
            .unwrap(),
            DirAction::Emit(2)
        );
        assert_eq!(
            merge_directories_action(&group(
                Some(dir("d", "n")),
                Some(dir("h", "n")),
                Some(dir("d", "n"))
            ))
            .unwrap(),
            DirAction::Emit(1)
        );
        assert_eq!(
            merge_directories_action(&group(
                Some(dir("d", "n")),
                Some(dir("h", "n")),
                Some(dir("r", "n"))
            ))
            .unwrap(),
            DirAction::Recurse { emit_slot: 1 }
        );
        // mask 6 (base is a file): the same as 7 for the directory decision.
        assert_eq!(
            merge_directories_action(&group(
                Some(file("base", "n")),
                Some(dir("h", "n")),
                Some(dir("r", "n"))
            ))
            .unwrap(),
            DirAction::Recurse { emit_slot: 1 }
        );
    }
}
