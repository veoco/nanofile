//! Server-side three-way merge of directory trees.
//!
//! `PUT /repo/{id}/commit/HEAD` merges instead of rejecting when the uploaded
//! commit's parent is no longer HEAD. The merge combines the client's tree with
//! the current HEAD against their common base and returns the root of the merged
//! tree. File contents never change: every emitted entry is a copy of a head or
//! remote entry, mutated only by a conflict rename or by a merged sub-directory
//! id. Entries are stored in descending name order, which is part of a
//! directory object's identity.

use std::collections::{BTreeMap, HashMap};

use sea_orm::DatabaseConnection;

use crate::domain::fs::sort_dirents_desc;
use crate::fs::core::store::store_fs_dir_object;
use crate::fs::core::traversal::{TreeGuard, max_tree_depth};
use crate::fs::core::tree::read_fs_dir_data;
use crate::repository::Repositories;
use base::common::{DirEntryData, FsDirData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use base::sanitize::{MAX_FILENAME_LEN, validate_filename};

/// Result of merging one directory tree.
pub struct MergeOutcome {
    /// Root of the merged tree; `EMPTY_SHA1` when nothing is left.
    pub merged_root: String,
    /// Whether a conflicting entry was renamed.
    pub conflict: bool,
    /// Whether an entry one side had deleted was kept because the other side
    /// changed it. The HTTP layer marks the corresponding activity paths.
    pub kept_deleted: bool,
    /// Number of directories the merge walked.
    pub visit_dirs: usize,
}

/// Merge `remote_root` (the tree the client uploaded) with the current
/// `head_root`, three-way against `base_root`.
///
/// `base_root == EMPTY_SHA1` means there is no common ancestor and is treated as
/// an empty tree. A missing or non-directory root is a hard error: by the time
/// the branch is advanced, every referenced object has been received.
pub async fn merge_trees(
    repos: &Repositories,
    db: &DatabaseConnection,
    repo_id: &str,
    base_root: &str,
    head_root: &str,
    remote_root: &str,
    remote_creator_name: &str,
) -> Result<MergeOutcome, AppError> {
    let base = load_dir(repos, repo_id, base_root).await?;
    let head = load_dir(repos, repo_id, head_root).await?;
    let remote = load_dir(repos, repo_id, remote_root).await?;

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

    let merged_root = Box::pin(merge_level(
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

async fn load_dir(repos: &Repositories, repo_id: &str, fs_id: &str) -> Result<FsDirData, AppError> {
    read_fs_dir_data(repos, repo_id, fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("merge: cannot read tree {fs_id}: {e}")))
}

struct MergeCtx<'a> {
    repos: &'a Repositories,
    db: &'a DatabaseConnection,
    repo_id: &'a str,
    /// Root of the client's tree, used to locate a conflict path's last modifier.
    remote_root: &'a str,
    /// Author of the client's commit, the fallback conflict-name suffix.
    remote_creator_name: &'a str,
    conflict: bool,
    kept_deleted: bool,
    visit_dirs: usize,
    guard: TreeGuard,
    /// `Some(nickname)` when the user exists, `None` when the email is unknown.
    nicknames: HashMap<String, Option<String>>,
}

#[derive(Debug, PartialEq, Eq)]
enum FileChoice {
    /// Emit nothing at this level.
    Drop,
    /// Emit one entry unchanged.
    Emit(DirEntryData),
    /// Keep the head entry when present, and rename the remote file.
    RenameRemote(Option<DirEntryData>),
    /// Rename the remote directory and emit the head entry.
    RenameRemoteDir(DirEntryData),
}

#[derive(Debug, PartialEq, Eq)]
enum DirChoice {
    /// Delete the directory from the merged tree.
    Drop,
    /// Emit this slot unchanged.
    Emit(usize),
    /// Recurse, then emit this slot with the merged child id.
    Recurse(usize),
}

/// Merge one level of the tree and store the resulting directory object.
async fn merge_level(
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

    for tree in trees.iter_mut().flatten() {
        sort_dirents_desc(&mut tree.dirents);
    }

    // Group the three lists by name. A directory may legitimately be shared by
    // several parents, but within one directory a name is unique; keeping the
    // first occurrence matches the cursor walk when a malformed object repeats
    // a name.
    let mut groups: BTreeMap<String, [Option<DirEntryData>; 3]> = BTreeMap::new();
    for (side, tree) in trees.iter().enumerate() {
        let Some(tree) = tree else { continue };
        for entry in &tree.dirents {
            let slots = groups
                .entry(entry.name.clone())
                .or_insert_with(|| [None, None, None]);
            if slots[side].is_none() {
                slots[side] = Some(entry.clone());
            }
        }
    }

    let mut merged: Vec<DirEntryData> = Vec::new();

    for (_, mut slots) in groups.into_iter().rev() {
        if slots
            .iter()
            .any(|s| s.as_ref().is_some_and(|e| is_regular(e.mode)))
        {
            match choose_file(&slots) {
                FileChoice::Drop => {}
                FileChoice::Emit(entry) => {
                    if slots[0].is_some() && (slots[1].is_none() != slots[2].is_none()) {
                        ctx.kept_deleted = true;
                    }
                    merged.push(entry);
                }
                FileChoice::RenameRemote(keep) => {
                    let remote = slots[2]
                        .as_ref()
                        .ok_or_else(|| AppError::Internal("merge: missing remote file".into()))?;
                    let new_name = ctx.conflict_file_name(basedir, &remote.name).await?;
                    if let Some(head) = keep {
                        merged.push(head);
                    }
                    let mut renamed = remote.clone();
                    renamed.name = new_name;
                    merged.push(renamed);
                    ctx.conflict = true;
                }
                FileChoice::RenameRemoteDir(head) => {
                    let remote = slots[2].as_mut().ok_or_else(|| {
                        AppError::Internal("merge: missing remote directory".into())
                    })?;
                    remote.name = ctx.conflict_dir_name(&remote.name);
                    merged.push(head);
                    ctx.conflict = true;
                }
            }
        }

        if slots
            .iter()
            .any(|s| s.as_ref().is_some_and(|e| is_dir(e.mode)))
        {
            match choose_dir(&slots)? {
                DirChoice::Drop => {}
                DirChoice::Emit(slot) => {
                    let other = if slot == 1 { 2 } else { 1 };
                    if slots[0].is_some() && slots[other].is_none() {
                        ctx.kept_deleted = true;
                    }
                    if let Some(entry) = &slots[slot] {
                        merged.push(entry.clone());
                    }
                }
                DirChoice::Recurse(emit_slot) => {
                    let other = if emit_slot == 1 { 2 } else { 1 };
                    if slots[other].is_none() {
                        ctx.kept_deleted = true;
                    }

                    let dirname = slots
                        .iter()
                        .flatten()
                        .rfind(|entry| is_dir(entry.mode))
                        .map(|entry| entry.name.clone())
                        .ok_or_else(|| {
                            AppError::Internal("merge: recursive action without a directory".into())
                        })?;
                    let child_basedir = format!("{basedir}{dirname}/");

                    let mut children: [Option<FsDirData>; 3] = [None, None, None];
                    for (side, entry) in slots.iter().enumerate() {
                        if let Some(entry) = entry
                            && is_dir(entry.mode)
                        {
                            children[side] =
                                Some(load_dir(ctx.repos, ctx.repo_id, &entry.id).await?);
                        }
                    }

                    let merged_root =
                        Box::pin(merge_level(ctx, depth + 1, children, &child_basedir)).await?;

                    let mut entry = slots[emit_slot].clone().ok_or_else(|| {
                        AppError::Internal("merge: missing directory to emit".into())
                    })?;
                    entry.id = merged_root;
                    merged.push(entry);
                }
            }
        }
    }

    let data = FsDirData {
        dirents: merged,
        obj_type: SEAF_METADATA_TYPE_DIR,
        version: 1,
    };
    store_fs_dir_object(ctx.db, ctx.repo_id, &data).await
}

fn as_file(slot: &Option<DirEntryData>) -> Option<&DirEntryData> {
    slot.as_ref().filter(|entry| is_regular(entry.mode))
}

/// File part of the merge table. The branches that inspect the opposite slot
/// for a directory presence are the D/F cases.
fn choose_file(slots: &[Option<DirEntryData>; 3]) -> FileChoice {
    let base = as_file(&slots[0]).cloned();
    let head = as_file(&slots[1]).cloned();
    let remote = as_file(&slots[2]).cloned();

    if let (Some(head), Some(remote)) = (&head, &remote) {
        if head.id == remote.id {
            return FileChoice::Emit(head.clone());
        }
        if base.as_ref().is_some_and(|entry| entry.id == head.id) {
            return FileChoice::Emit(remote.clone());
        }
        if base.as_ref().is_some_and(|entry| entry.id == remote.id) {
            return FileChoice::Emit(head.clone());
        }
        return FileChoice::RenameRemote(Some(head.clone()));
    }

    if let (Some(base), None, Some(remote)) = (&base, &head, &remote) {
        if base.id == remote.id {
            return FileChoice::Drop;
        }
        if slots[1].is_some() {
            // Head replaced the file with a directory while remote changed it:
            // keep the directory name, rename the remote file.
            return FileChoice::RenameRemote(None);
        }
        return FileChoice::Emit(remote.clone());
    }

    if let (Some(base), Some(head), None) = (&base, &head, &remote) {
        if base.id == head.id {
            return FileChoice::Drop;
        }
        if slots[2].is_some() {
            // Remote replaced the file with a directory while head changed it:
            // emit head's file and rename remote's directory.
            return FileChoice::RenameRemoteDir(head.clone());
        }
        return FileChoice::Emit(head.clone());
    }

    if let (None, None, Some(remote)) = (&base, &head, &remote) {
        if slots[1].is_none() {
            return FileChoice::Emit(remote.clone());
        }
        if slots[0]
            .as_ref()
            .is_some_and(|entry| entry.id == slots[1].as_ref().unwrap().id)
        {
            // The head directory is unchanged, so the merge drops it and the
            // remote file takes the name.
            return FileChoice::Emit(remote.clone());
        }
        // Head added a directory where remote added a file with the same name.
        return FileChoice::RenameRemote(None);
    }

    if let (None, Some(head), None) = (&base, &head, &remote) {
        if slots[2].is_none() {
            return FileChoice::Emit(head.clone());
        }
        if slots[0]
            .as_ref()
            .is_some_and(|entry| entry.id == slots[2].as_ref().unwrap().id)
        {
            return FileChoice::Emit(head.clone());
        }
        return FileChoice::RenameRemoteDir(head.clone());
    }

    FileChoice::Drop
}

/// Directory part of the merge table, as a 3-bit mask over base/head/remote.
fn choose_dir(slots: &[Option<DirEntryData>; 3]) -> Result<DirChoice, AppError> {
    let mut mask = 0usize;
    for (side, slot) in slots.iter().enumerate() {
        if slot.as_ref().is_some_and(|entry| is_dir(entry.mode)) {
            mask |= 1 << side;
        }
    }

    let base = slots[0].as_ref();
    let head = slots[1].as_ref();
    let remote = slots[2].as_ref();

    let choice = match mask {
        0 => return Err(AppError::Internal("merge: empty directory group".into())),
        1 => DirChoice::Drop,
        2 => DirChoice::Emit(1),
        3 => {
            if base.unwrap().id == head.unwrap().id {
                DirChoice::Drop
            } else {
                DirChoice::Recurse(1)
            }
        }
        4 => DirChoice::Emit(2),
        5 => {
            if base.unwrap().id == remote.unwrap().id {
                DirChoice::Drop
            } else {
                DirChoice::Recurse(2)
            }
        }
        6 | 7 => {
            if head.unwrap().id == remote.unwrap().id {
                DirChoice::Emit(1)
            } else if base.is_some_and(|entry| entry.id == head.unwrap().id) {
                DirChoice::Emit(2)
            } else if base.is_some_and(|entry| entry.id == remote.unwrap().id) {
                DirChoice::Emit(1)
            } else {
                DirChoice::Recurse(1)
            }
        }
        _ => return Err(AppError::Internal("merge: invalid directory group".into())),
    };

    Ok(choice)
}

impl MergeCtx<'_> {
    /// Nickname of an email, or the email itself when the user is unknown.
    async fn nickname(&mut self, email: &str) -> Result<String, AppError> {
        if let Some(cached) = self.nicknames.get(email) {
            return Ok(cached.clone().unwrap_or_else(|| email.to_string()));
        }
        let nickname = self
            .repos
            .user
            .find_by_email(email)
            .await?
            .map(|user| user.nickname());
        self.nicknames.insert(email.to_string(), nickname.clone());
        Ok(nickname.unwrap_or_else(|| email.to_string()))
    }

    /// Last modifier and mtime of `path` in the client's tree.
    ///
    /// `Ok(None)` means the path is not there; `Err` means the walk could not be
    /// completed, and the caller falls back to the commit author and now.
    async fn remote_modifier_mtime(&self, path: &str) -> Result<Option<(String, i64)>, AppError> {
        let (parent, filename) = match path.rsplit_once('/') {
            Some((parent, filename)) => (parent, filename),
            None => ("", path),
        };

        let mut dir_id = self.remote_root.to_string();
        for segment in parent.split('/').filter(|segment| !segment.is_empty()) {
            let dir = load_dir(self.repos, self.repo_id, &dir_id).await?;
            let entry = dir
                .dirents
                .iter()
                .find(|entry| entry.name == segment && is_dir(entry.mode))
                .ok_or_else(|| AppError::NotFound("merge: conflict path not found".into()))?;
            dir_id = entry.id.clone();
        }

        let dir = load_dir(self.repos, self.repo_id, &dir_id).await?;
        Ok(dir
            .dirents
            .iter()
            .find(|entry| entry.name == filename)
            .map(|entry| (entry.modifier.clone(), entry.mtime)))
    }

    async fn conflict_file_name(&mut self, basedir: &str, name: &str) -> Result<String, AppError> {
        let path = format!("{basedir}{name}");
        let (modifier, mtime) = match self.remote_modifier_mtime(&path).await {
            Ok(Some((modifier, mtime))) => (Some(modifier), mtime),
            Ok(None) => (None, 0),
            Err(_) => (
                Some(self.remote_creator_name.to_string()),
                chrono::Utc::now().timestamp(),
            ),
        };

        let nickname = match modifier {
            Some(email) => Some(self.nickname(&email).await?),
            None => None,
        };
        Ok(conflict_path(
            name,
            nickname.as_deref(),
            &format_local_timestamp(mtime),
        ))
    }

    fn conflict_dir_name(&self, name: &str) -> String {
        conflict_path(
            name,
            Some(self.remote_creator_name),
            &format_local_timestamp(chrono::Utc::now().timestamp()),
        )
    }
}

/// Marker prefix of a conflict name, before the modifier (see `conflict_path`).
const CONFLICT_PREFIX: &str = " (SFConflict ";

/// Rename a conflicted entry to `"<stem> (SFConflict <modifier> <timestamp>)<ext>"`.
///
/// The modifier is a nickname or email from the library's user table (or the
/// commit author), i.e. attacker-influenced data. It is stripped of control
/// characters, path separators and characters Win32 forbids, so the generated
/// name can neither escape the directory nor carry an entry a Windows client
/// cannot materialize. The name is bounded to [`MAX_FILENAME_LEN`] bytes by
/// shortening the stem (extension and marker survive).
///
/// If the result is still not a valid filename — the original name itself is
/// malformed, e.g. it contains `?` — the modifier-free shape is tried, and a
/// name that cannot be made valid keeps its original name instead of losing the
/// entry (the merge must not drop data); the failure is logged for the operator.
fn conflict_path(path: &str, modifier: Option<&str>, timestamp: &str) -> String {
    let (stem, extension) = match path.rfind('.') {
        // The dot belongs to the extension, like the original `format!`: a
        // dotfile (".bashrc") has an empty stem and keeps its leading dot.
        Some(dot) => (&path[..dot], Some(&path[dot..])),
        None => (path, None),
    };
    let extension_len = extension.map_or(0, str::len);
    let timestamp_overhead = CONFLICT_PREFIX.len() + " ".len() + timestamp.len() + ")".len();

    let build = |label: &str| -> String {
        let tail_len = label.len() + extension_len;
        let stem_budget = MAX_FILENAME_LEN.saturating_sub(tail_len);
        let mut name = String::with_capacity(stem_budget + tail_len);
        name.push_str(truncate_utf8(stem, stem_budget));
        name.push_str(label);
        if let Some(extension) = extension {
            name.push_str(extension);
        }
        name
    };

    if let Some(modifier) = modifier {
        // Reserve room for the marker and the extension; only then take from
        // the modifier, so a long nickname cannot push the name over the limit.
        let modifier_budget = MAX_FILENAME_LEN
            .saturating_sub(timestamp_overhead)
            .saturating_sub(extension_len);
        let clean = sanitize_conflict_modifier(modifier, modifier_budget);
        let candidate = build(&format!("{CONFLICT_PREFIX}{clean} {timestamp})"));
        if validate_filename(&candidate).is_ok() {
            return candidate;
        }
    }

    let candidate = build(&format!("{CONFLICT_PREFIX}{timestamp})"));
    if validate_filename(&candidate).is_ok() {
        return candidate;
    }

    tracing::error!(
        path,
        modifier = modifier.unwrap_or_default(),
        "merge: cannot build a valid conflict filename, keeping the original name"
    );
    path.to_string()
}

/// Strip everything a generated conflict name must not contain from `modifier`.
///
/// Removed rather than escaped: the conflict suffix is user-facing, and a name
/// like `a (SFConflict ../evil 2026-…).txt` would otherwise leak the traversal
/// attempt into every client's conflict list.
fn sanitize_conflict_modifier(modifier: &str, max_bytes: usize) -> String {
    let cleaned: String = modifier
        .chars()
        .filter(|ch| {
            !ch.is_control()
                && !matches!(
                    ch,
                    '/' | '\\' | '\'' | '"' | '<' | '>' | ':' | '*' | '?' | '|'
                )
        })
        .collect();
    // The modifier sits before " <timestamp>)", so a trailing dot/space does not
    // end the name; it is still trimmed so repeated separators do not pile up.
    let cleaned = cleaned.trim_end_matches([' ', '.']);
    truncate_utf8(cleaned, max_bytes).to_string()
}

/// Prefix of `s` at most `max_bytes` long, never splitting a UTF-8 character.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn format_local_timestamp(seconds: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(seconds, 0).single() {
        Some(time) => time.format("%Y-%m-%d-%H-%M-%S").to_string(),
        None => seconds.to_string(),
    }
}

const S_IFMT: i32 = 0o170000;
const S_IFREG_TYPE: i32 = 0o100000;
const S_IFDIR_TYPE: i32 = 0o040000;

fn is_regular(mode: i32) -> bool {
    mode & S_IFMT == S_IFREG_TYPE
}

fn is_dir(mode: i32) -> bool {
    mode & S_IFMT == S_IFDIR_TYPE
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

    fn slots(
        base: Option<DirEntryData>,
        head: Option<DirEntryData>,
        remote: Option<DirEntryData>,
    ) -> [Option<DirEntryData>; 3] {
        [base, head, remote]
    }

    #[test]
    fn conflict_path_keeps_the_last_extension() {
        let timestamp = "2026-09-17-10-20-30";
        assert_eq!(
            conflict_path("a.txt", Some("Alice"), timestamp),
            "a (SFConflict Alice 2026-09-17-10-20-30).txt"
        );
        assert_eq!(
            conflict_path("a.b.c", Some("Alice"), timestamp),
            "a.b (SFConflict Alice 2026-09-17-10-20-30).c"
        );
        assert_eq!(
            conflict_path("report", Some("Alice"), timestamp),
            "report (SFConflict Alice 2026-09-17-10-20-30)"
        );
        assert_eq!(
            conflict_path("a.txt", None, timestamp),
            "a (SFConflict 2026-09-17-10-20-30).txt"
        );
        assert_eq!(
            conflict_path(".bashrc", Some("Alice"), timestamp),
            " (SFConflict Alice 2026-09-17-10-20-30).bashrc"
        );
        assert_eq!(
            conflict_path("a.txt", Some(""), timestamp),
            "a (SFConflict  2026-09-17-10-20-30).txt"
        );
    }

    /// The modifier comes from a nickname/email (and the creator name from the
    /// client), so anything that would break the filename is removed before it
    /// is embedded.
    #[test]
    fn conflict_path_sanitizes_the_modifier() {
        let timestamp = "2026-09-17-10-20-30";
        assert_eq!(
            conflict_path("a.txt", Some("in/v:a*l?u\"e\n"), timestamp),
            "a (SFConflict invalue 2026-09-17-10-20-30).txt"
        );
        assert_eq!(
            conflict_path("a.txt", Some("../evil\\dir"), timestamp),
            "a (SFConflict ..evildir 2026-09-17-10-20-30).txt"
        );
        // A normal nickname/email keeps the format the desktop clients show.
        assert_eq!(
            conflict_path("report.docx", Some("alice@example.com"), timestamp),
            "report (SFConflict alice@example.com 2026-09-17-10-20-30).docx"
        );
    }

    /// No conflict name may exceed the server's own filename limit: a long
    /// modifier is truncated and a long stem is shortened, keeping the marker
    /// and the extension.
    #[test]
    fn conflict_path_stays_within_the_filename_limit() {
        let timestamp = "2026-09-17-10-20-30";

        let long_modifier = "m".repeat(1000);
        let out = conflict_path("a.txt", Some(&long_modifier), timestamp);
        assert_eq!(out.len(), MAX_FILENAME_LEN);
        assert!(
            out.ends_with(").txt"),
            "marker/extension must survive: {out}"
        );
        assert!(validate_filename(&out).is_ok());

        let long_name = format!("{}.txt", "n".repeat(240));
        let out = conflict_path(&long_name, Some("Alice"), timestamp);
        assert_eq!(out.len(), MAX_FILENAME_LEN);
        assert!(out.starts_with(&"n".repeat(200)), "got {out}");
        assert!(out.ends_with(").txt"), "got {out}");
        assert!(validate_filename(&out).is_ok());
    }

    /// A malformed original name cannot be renamed into a valid one; the entry
    /// must survive under its original name rather than be dropped.
    #[test]
    fn conflict_path_falls_back_when_the_name_cannot_be_fixed() {
        let timestamp = "2026-09-17-10-20-30";
        // An invalid character in the original name makes every shape invalid.
        assert_eq!(
            conflict_path("bad?.txt", Some("Alice"), timestamp),
            "bad?.txt"
        );
        // An extension longer than the whole budget leaves no room for the
        // marker, so the original name is kept as well.
        let huge_extension = format!("a.{}", "x".repeat(250));
        assert_eq!(
            conflict_path(&huge_extension, None, timestamp),
            huge_extension
        );
    }

    #[test]
    fn files_match_emit_head() {
        assert_eq!(
            choose_file(&slots(
                Some(file("old", "f")),
                Some(file("new", "f")),
                Some(file("new", "f"))
            )),
            FileChoice::Emit(file("new", "f"))
        );
    }

    #[test]
    fn file_changed_on_one_side_takes_that_side() {
        assert_eq!(
            choose_file(&slots(
                Some(file("base", "f")),
                Some(file("base", "f")),
                Some(file("remote", "f"))
            )),
            FileChoice::Emit(file("remote", "f"))
        );
        assert_eq!(
            choose_file(&slots(
                Some(file("base", "f")),
                Some(file("head", "f")),
                Some(file("base", "f"))
            )),
            FileChoice::Emit(file("head", "f"))
        );
    }

    #[test]
    fn file_changed_on_both_sides_renames_remote() {
        assert_eq!(
            choose_file(&slots(
                Some(file("base", "f")),
                Some(file("head", "f")),
                Some(file("remote", "f"))
            )),
            FileChoice::RenameRemote(Some(file("head", "f")))
        );
    }

    #[test]
    fn delete_versus_modify_keeps_the_changed_side() {
        assert_eq!(
            choose_file(&slots(
                Some(file("base", "f")),
                None,
                Some(file("remote", "f"))
            )),
            FileChoice::Emit(file("remote", "f"))
        );
        assert_eq!(
            choose_file(&slots(
                Some(file("base", "f")),
                Some(file("head", "f")),
                None
            )),
            FileChoice::Emit(file("head", "f"))
        );
    }

    #[test]
    fn delete_unchanged_side_drops_the_file() {
        assert_eq!(
            choose_file(&slots(
                Some(file("same", "f")),
                None,
                Some(file("same", "f"))
            )),
            FileChoice::Drop
        );
        assert_eq!(
            choose_file(&slots(
                Some(file("same", "f")),
                Some(file("same", "f")),
                None
            )),
            FileChoice::Drop
        );
        assert_eq!(
            choose_file(&slots(Some(file("base", "f")), None, None)),
            FileChoice::Drop
        );
    }

    #[test]
    fn directory_mask_table() {
        assert_eq!(
            choose_dir(&slots(Some(dir("b", "d")), None, None)).unwrap(),
            DirChoice::Drop
        );
        assert_eq!(
            choose_dir(&slots(None, Some(dir("h", "d")), None)).unwrap(),
            DirChoice::Emit(1)
        );
        assert_eq!(
            choose_dir(&slots(None, None, Some(dir("r", "d")))).unwrap(),
            DirChoice::Emit(2)
        );
        assert_eq!(
            choose_dir(&slots(Some(dir("b", "d")), Some(dir("b", "d")), None)).unwrap(),
            DirChoice::Drop
        );
        assert_eq!(
            choose_dir(&slots(Some(dir("b", "d")), Some(dir("h", "d")), None)).unwrap(),
            DirChoice::Recurse(1)
        );
        assert_eq!(
            choose_dir(&slots(Some(dir("b", "d")), None, Some(dir("b", "d")))).unwrap(),
            DirChoice::Drop
        );
        assert_eq!(
            choose_dir(&slots(Some(dir("b", "d")), None, Some(dir("r", "d")))).unwrap(),
            DirChoice::Recurse(2)
        );
        assert_eq!(
            choose_dir(&slots(None, Some(dir("h", "d")), Some(dir("h", "d")))).unwrap(),
            DirChoice::Emit(1)
        );
        assert_eq!(
            choose_dir(&slots(
                Some(dir("b", "d")),
                Some(dir("b", "d")),
                Some(dir("r", "d"))
            ))
            .unwrap(),
            DirChoice::Emit(2)
        );
        assert_eq!(
            choose_dir(&slots(
                Some(dir("b", "d")),
                Some(dir("h", "d")),
                Some(dir("b", "d"))
            ))
            .unwrap(),
            DirChoice::Emit(1)
        );
        assert_eq!(
            choose_dir(&slots(
                Some(dir("b", "d")),
                Some(dir("h", "d")),
                Some(dir("r", "d"))
            ))
            .unwrap(),
            DirChoice::Recurse(1)
        );
        assert!(choose_dir(&slots(None, None, None)).is_err());
    }

    #[test]
    fn df_conflicts_are_detected() {
        // Head replaced the file with a directory; remote changed the file.
        assert_eq!(
            choose_file(&slots(
                Some(file("base", "f")),
                Some(dir("headdir", "f")),
                Some(file("remote", "f"))
            )),
            FileChoice::RenameRemote(None)
        );
        // Remote replaced the file with a directory; head changed the file.
        assert_eq!(
            choose_file(&slots(
                Some(file("base", "f")),
                Some(file("head", "f")),
                Some(dir("remotedir", "f"))
            )),
            FileChoice::RenameRemoteDir(file("head", "f"))
        );
    }

    #[test]
    fn descending_order_is_byte_order() {
        let mut entries = vec![file("1", "a.txt"), dir("2", "b"), file("3", "C.txt")];
        sort_dirents_desc(&mut entries);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["b", "a.txt", "C.txt"]);
    }
}
