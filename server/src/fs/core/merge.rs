//! Server-side three-way merge of FS trees (`common/merge-new.c` upstream).
//!
//! `PUT /repo/{id}/commit/HEAD` is the last step of a client's upload. When the
//! uploaded commit's parent is no longer the repo's HEAD — two devices synced the
//! same library at the same time — upstream does **not** reject the upload: it
//! merges the uploaded tree with the current HEAD on the server
//! (`fast_forward_or_merge()`, `server/http-server.c`) and records the result as a
//! merge commit with two parents. This module is that merge.
//!
//! Faithfulness notes, all of which have bitten upstream readers before:
//!
//! * The merge never changes file *contents*. Every output entry is a copy of a
//!   dirent that exists in the head tree or in the uploaded ("remote") tree; the
//!   only mutation is a rename when a conflict has to preserve both versions
//!   (`SFConflict` names) and the id of a recursively merged sub-directory.
//! * Directory entry lists are stored in **descending** name order
//!   (`compare_dirents()` is `strcmp(b, a)` in `common/fs-mgr.c`), and the merge
//!   walk depends on that order to match names across the three trees. We sort
//!   each input defensively: nanofile also writes directory objects from its own
//!   REST paths, where nothing guarantees the order. Sorting the *inputs* is
//!   safe because a directory object's id is the hash of its serialized form —
//!   re-sorting a loaded object and writing it back would produce a different id.
//! * `threeway_merge()` renames the remote entry **in place** and
//!   `merge_directories()` then sees the renamed slot. The order of the two
//!   phases and the in-place rename are reproduced here because a D/F conflict
//!   (`file → dir` / `dir → file`) depends on it.
//! * The conflict name embeds the last modifier of the *remote* tree's copy of
//!   the file and that entry's mtime, formatted in local time
//!   (`gen_conflict_path()` / `get_file_modifier_mtime()` in `common/vc-common.c`).

use std::collections::HashMap;

use sea_orm::DatabaseConnection;

use crate::fs::core::store::store_fs_dir_object;
use crate::fs::core::traversal::{TreeGuard, max_tree_depth};
use crate::fs::core::tree::read_fs_dir_data;
use crate::repository::Repositories;
use base::common::{DirEntryData, FsDirData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;

/// `S_IFMT`, `S_IFREG` and `S_IFDIR` type bits, used like C's `S_ISREG()` /
/// `S_ISDIR()`. `base::common::S_IFREG` carries the permission bits
/// (`0100644`), so it cannot be used as a mask here.
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
    /// Root of the merged tree. `EMPTY_SHA1` when nothing is left in it, in
    /// which case no object is stored (like `seaf_dir_save()`).
    pub merged_root: String,
    /// Whether the merge had to rename a conflicting entry, i.e. upstream's
    /// `opt.conflict` and the `conflict` flag of the merge commit.
    pub conflict: bool,
    /// Number of directories the merge walked (upstream's `opt.visit_dirs`,
    /// which it logs for debugging).
    pub visit_dirs: usize,
}

/// Merge `remote_root` (the tree the client just uploaded, whose base commit is
/// `base_root`) with the repository's current `head_root`, three-way against
/// `base_root` as the common ancestor.
///
/// `base_root == EMPTY_SHA1` means "there is no common ancestor" and is treated
/// as an empty tree — upstream cannot reach that state because it creates a
/// "Created library" commit for every new library, while nanofile starts a
/// library with no HEAD at all.
///
/// A missing or non-directory object is a hard error (upstream returns -1, which
/// the callback turns into a 500): by the time the branch is advanced, every
/// object the three trees reference has been received.
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
        visit_dirs: 0,
        guard: TreeGuard::new(),
        nicknames: HashMap::new(),
    };

    // Upstream requires all three roots to exist (`seaf_merge_trees()`); an
    // `EMPTY_SHA1` root is the empty tree and is synthesized by
    // `read_fs_dir_data`.
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
    /// Root of the remote (client-supplied) commit, used to look up the last
    /// modifier of a conflicting file.
    remote_root: &'a str,
    remote_creator_name: &'a str,
    conflict: bool,
    visit_dirs: usize,
    guard: TreeGuard,
    /// modifier email → nickname, cached for the whole merge (upstream keeps the
    /// same cache in `opt->email_to_nickname`). `None` = no user with that
    /// email, in which case the email itself is used.
    nicknames: HashMap<String, Option<String>>,
}

/// The file-slot decision for one name group (upstream `threeway_merge()`,
/// n = 3). Pure so the decision table can be unit-tested without a database.
#[derive(Debug, PartialEq, Eq)]
enum FileAction {
    /// Emit these entries verbatim.
    Emit(Vec<DirEntryData>),
    /// Conflict: keep `keep` (the head side, absent in the D/F case) and rename
    /// `remote` with a `SFConflict` name.
    ConflictFile {
        keep: Option<DirEntryData>,
        remote: DirEntryData,
    },
    /// D/F conflict in which the *directory* in slot `rename_slot` is renamed
    /// (`merge_conflict_dirname()`); `emit` is the head-side file.
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
    /// Merge the sub-directories recursively, then emit slot `emit_slot` with
    /// the merged id patched in.
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
            // Unchanged in head, changed in remote.
            return FileAction::Emit(vec![remote.clone()]);
        }
        if base.as_ref().is_some_and(|b| b.id == remote.id) {
            // Unchanged in remote, changed in head.
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
            // Deleted in head, unchanged in remote: stay deleted.
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
            // Deleted in remote, unchanged in head: stay deleted.
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
        // Deleted in remote and changed in head: keep head's version.
        return FileAction::Emit(vec![head.clone()]);
    }

    if let (None, None, Some(remote)) = (&base, &head, &remote) {
        if d1.is_none() {
            // Added in remote.
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
            // Added in head.
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

    // `base && !head && !remote`: deleted in both — nothing to emit. The
    // remaining combinations cannot happen: a name group always has at least
    // one slot, and every case with a present slot is handled above.
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
        // Head and remote are not directories: nothing to merge.
        1 => DirAction::Delete,
        // Only head is a directory.
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
        // Only remote is a directory.
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

/// Descending name order, i.e. upstream's `compare_dirents()` = `strcmp(b, a)`
/// and the order directory entries are serialized in.
fn sort_dirents_desc(dirents: &mut [DirEntryData]) {
    dirents.sort_by(|a, b| b.name.cmp(&a.name));
}

/// `gen_conflict_path()` (`common/vc-common.c:584`).
///
/// The time string is passed in so the pure formatting (in particular the
/// leading-dot and extension-splitting rules) can be unit-tested without a
/// timezone.
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
        // `localtime()` would hand `strftime` a NULL `tm` and crash; there is no
        // sensible name to build, so fall back to the raw timestamp.
        None => mtime.to_string(),
    }
}

impl MergeCtx<'_> {
    /// Nickname of a `modifier` email, i.e. seahub's `email2nickname()`
    /// (`get_nickname_by_modifier()`): the nicknames the conflict filename
    /// embeds. Falls back to the email itself, which is what upstream does when
    /// the seahub lookup fails.
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

    /// `get_file_modifier_mtime()` for `version > 0`
    /// (`get_file_modifier_mtime_v1()`, `common/vc-common.c:499`): the last
    /// modifier and mtime of `path` in the *remote* tree.
    ///
    /// `Ok(None)` is upstream's "walk succeeded but the entry is not there"
    /// (a NULL modifier with mtime 0); `Err` is the "walk failed" case, for
    /// which the caller substitutes the committer and the current time.
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

    /// `merge_conflict_dirname()` (`common/merge-new.c:86`): always the remote
    /// commit's author and the current time.
    fn conflict_dir_name(&self, name: &str) -> String {
        gen_conflict_path(
            name,
            Some(self.remote_creator_name),
            &format_local_time(chrono::Utc::now().timestamp()),
        )
    }
}

/// `merge_trees_recursive()` (`common/merge-new.c:597`): merge the three
/// directories at one level and return the merged directory's id.
///
/// `basedir` is the merged result's path prefix for this level (no leading
/// slash, `""` at the root, ending with `/` below the root); it is only used to
/// build conflict names.
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
        // "Largest" name among the three cursors, i.e. the next name in
        // descending order.
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
            match threeway_merge_files(&slots) {
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
                    // Rename in place: `merge_directories()` below sees the new
                    // name (and uses it to build the children's `basedir`).
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
                    if let Some(dent) = &slots[slot] {
                        merged.push(dent.clone());
                    }
                }
                DirAction::Recurse { emit_slot } => {
                    // Upstream takes the last directory slot's name as the
                    // child prefix; every slot here shares the same name, but a
                    // slot renamed above is the one that ends up in the result.
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
        // Unchanged in remote -> stays deleted.
        assert_eq!(
            threeway_merge_files(&group(
                Some(file("base", "f")),
                None,
                Some(file("base", "f"))
            )),
            FileAction::Nothing
        );
        // Changed in remote -> keep remote's version.
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
        // Remote turned the name into a directory -> head's file kept, remote's
        // directory renamed.
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
        // Added in remote, no head entry at all.
        assert_eq!(
            threeway_merge_files(&group(None, None, Some(file("remote", "f")))),
            FileAction::Emit(vec![file("remote", "f")])
        );
        // Same, but base is a directory whose contents head left unchanged, so
        // `merge_directories()` deletes the directory and remote's file keeps
        // the name.
        assert_eq!(
            threeway_merge_files(&group(
                Some(dir("samedir", "f")),
                Some(dir("samedir", "f")),
                Some(file("remote", "f"))
            )),
            FileAction::Emit(vec![file("remote", "f")])
        );
        // Added in head.
        assert_eq!(
            threeway_merge_files(&group(None, Some(file("head", "f")), None)),
            FileAction::Emit(vec![file("head", "f")])
        );
        // Same, but remote left the directory it replaced unchanged.
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
