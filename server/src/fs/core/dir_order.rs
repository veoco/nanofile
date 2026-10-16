//! Directory-object order verification and repair (`nanofile verify-dir-order`).
//!
//! `fs_id` is the SHA-1 of a directory object's serialized form, so the order of
//! its `dirents` **is** part of the object's identity. Official clients always
//! write entries in descending name order (upstream `compare_dirents()` is
//! `strcmp(b, a)`, and `daemon/vc-utils.c` rebuilds every directory from the
//! index and sorts it), and they read a stored v1 object back verbatim
//! (`seaf_fs_manager_get_seafdir_sorted()` only sorts version 0). A directory
//! stored in any other order therefore carries an id no client can reproduce,
//! and the next local write in that directory makes the two sides disagree
//! about a directory neither of them changed.
//!
//! [`DirOrder::run`] walks every directory reachable from a library's HEAD and
//! reports the objects whose stored order is not canonical, plus structural
//! findings it deliberately does not repair (dangling `dirent` ids, duplicate
//! names, unknown modes). With `repair` it appends one ordinary commit per
//! affected library whose root is the canonical form of the current tree:
//! nothing in history is rewritten, old objects stay readable, and clients pick
//! the fix up as a normal update. Running it twice changes nothing the second
//! time.

use std::collections::HashMap;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use base::common::{CommitData, EMPTY_SHA1, FsDirData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::entity::{commit, fs_object, repo};

use crate::domain;
use crate::fs::core::store::store_fs_dir_object;
use crate::fs::core::traversal::max_tree_depth;

/// `S_IFMT` / `S_IFDIR` / `S_IFREG` type bits, used like C's `S_ISDIR()` /
/// `S_ISREG()`. `base::common::S_IFREG` carries permission bits (`0100644`), so
/// it cannot be used as a mask here.
const S_IFMT: i32 = 0o170000;
const S_IFDIR_TYPE: i32 = 0o040000;
const S_IFREG_TYPE: i32 = 0o100000;

/// Description of the one commit a repair run may append.
const NORMALIZE_DESCRIPTION: &str = "Normalize directory entry order";

/// A directory object stored in an order no official client would compute.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnorderedDir {
    pub repo_id: String,
    /// Path in the library's current tree (the first path the walk reached it
    /// by; a directory object may be shared by several paths).
    pub path: String,
    pub stored_id: String,
    /// Id of the same entries serialized in canonical (descending) order.
    pub canonical_id: String,
    pub dirents: usize,
}

/// Something the scan reports but never repairs on its own.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StructuralFinding {
    pub repo_id: String,
    pub path: String,
    /// `dangling-dirent`, `duplicate-name` or `unknown-mode`.
    pub kind: String,
    pub detail: String,
}

/// A library a repair run gave a normalizing commit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoRepair {
    pub repo_id: String,
    pub head_before: String,
    pub head_after: String,
    pub dirs_rewritten: usize,
}

/// What one verification pass found.
#[derive(Debug, Default, serde::Serialize)]
pub struct DirOrderReport {
    pub repos_scanned: usize,
    pub dirs_checked: u64,
    pub unordered: Vec<UnorderedDir>,
    pub other: Vec<StructuralFinding>,
    pub repairs: Vec<RepoRepair>,
    /// Failures: a tree that could not be walked, or a repair whose head swap
    /// lost a race. The corresponding library was left untouched.
    pub repair_errors: Vec<String>,
}

impl DirOrderReport {
    /// Whether the pass found anything an operator must act on.
    pub fn has_findings(&self) -> bool {
        !self.unordered.is_empty() || !self.other.is_empty() || !self.repair_errors.is_empty()
    }

    /// Print the report: the machine-readable form when `json`, a summary
    /// otherwise.
    pub fn print(&self, json: bool) {
        if json {
            match serde_json::to_string_pretty(self) {
                Ok(text) => println!("{text}"),
                Err(e) => eprintln!("could not serialize the report: {e}"),
            }
            return;
        }

        println!("libraries scanned:            {}", self.repos_scanned);
        println!("directory objects checked:    {}", self.dirs_checked);
        println!(
            "directories in non-canonical order: {}",
            self.unordered.len()
        );
        for dir in &self.unordered {
            println!(
                "  - {}{} (stored {}, canonical {}, {} dirent(s))",
                dir.repo_id,
                dir.path,
                short(&dir.stored_id),
                short(&dir.canonical_id),
                dir.dirents
            );
        }
        println!("other findings:               {}", self.other.len());
        for finding in &self.other {
            println!(
                "  - {}{} [{}] {}",
                finding.repo_id, finding.path, finding.kind, finding.detail
            );
        }
        for repair in &self.repairs {
            println!(
                "repaired {}: {} -> {} ({} directory object(s) rewritten)",
                repair.repo_id,
                short(&repair.head_before),
                short(&repair.head_after),
                repair.dirs_rewritten
            );
        }
        for error in &self.repair_errors {
            println!("  ! {error}");
        }
    }
}

/// First bytes of an id, for the human-readable report.
fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

/// The `verify-dir-order` command.
pub struct DirOrder;

impl DirOrder {
    /// Check every library (or `repo_filter`) and, when `repair`, append a
    /// normalizing commit to each affected one.
    pub async fn run(
        db: &DatabaseConnection,
        repo_filter: Option<&str>,
        repair: bool,
    ) -> Result<DirOrderReport, AppError> {
        let repos: Vec<repo::Model> = match repo_filter {
            Some(id) => {
                repo::Entity::find()
                    .filter(repo::Column::Id.eq(id))
                    .all(db)
                    .await?
            }
            None => repo::Entity::find().all(db).await?,
        };
        if repos.is_empty()
            && let Some(id) = repo_filter
        {
            return Err(AppError::NotFound(format!("library not found: {id}")));
        }

        let mut report = DirOrderReport {
            repos_scanned: repos.len(),
            ..Default::default()
        };

        for repo_row in &repos {
            let Some(head_id) = repo_row
                .head_commit_id
                .as_deref()
                .filter(|head| *head != EMPTY_SHA1)
            else {
                continue;
            };
            let Some(head_commit) = find_commit(db, &repo_row.id, head_id).await? else {
                report.repair_errors.push(format!(
                    "{}: HEAD commit {} is missing",
                    repo_row.id, head_id
                ));
                continue;
            };

            let mut checker = Checker {
                db,
                repo_id: &repo_row.id,
                rewrite: repair,
                memo: HashMap::new(),
                rewritten: 0,
            };
            let new_root = match checker
                .canonicalize(&head_commit.root_id, "/", 0, &mut report)
                .await
            {
                Ok(root) => root,
                Err(e) => {
                    report.repair_errors.push(format!("{}: {e}", repo_row.id));
                    continue;
                }
            };

            if !repair || new_root == head_commit.root_id {
                continue;
            }

            let rewrites = checker.rewritten;
            match append_normalizing_commit(db, repo_row, &head_commit, &new_root).await {
                Ok(Some(commit_id)) => report.repairs.push(RepoRepair {
                    repo_id: repo_row.id.clone(),
                    head_before: head_id.to_string(),
                    head_after: commit_id,
                    dirs_rewritten: rewrites,
                }),
                Ok(None) => report.repair_errors.push(format!(
                    "{}: HEAD moved while repairing; nothing was committed, re-run",
                    repo_row.id
                )),
                Err(e) => report
                    .repair_errors
                    .push(format!("{}: could not commit: {e}", repo_row.id)),
            }
        }

        Ok(report)
    }
}

/// Walks a library tree once, recording what it finds and returning each
/// directory's canonical id.
struct Checker<'a> {
    db: &'a DatabaseConnection,
    repo_id: &'a str,
    /// Insert the canonical objects; when false only compute the ids.
    rewrite: bool,
    /// old object id -> canonical id, so a shared subtree is walked once.
    memo: HashMap<String, String>,
    /// Objects whose canonical id differs from the stored one.
    rewritten: usize,
}

impl Checker<'_> {
    /// Canonical id of the directory object `fs_id` at `path`.
    async fn canonicalize(
        &mut self,
        fs_id: &str,
        path: &str,
        depth: usize,
        report: &mut DirOrderReport,
    ) -> Result<String, AppError> {
        if fs_id == EMPTY_SHA1 {
            // The empty directory: nothing stored, nothing to rewrite.
            return Ok(EMPTY_SHA1.to_string());
        }
        if let Some(new_id) = self.memo.get(fs_id) {
            return Ok(new_id.clone());
        }
        if depth > max_tree_depth() {
            return Err(AppError::BadRequest(format!(
                "{path} is deeper than the maximum directory depth ({})",
                max_tree_depth()
            )));
        }

        let Some(model) = find_object(self.db, self.repo_id, fs_id).await? else {
            return Err(AppError::NotFound(format!("fs_object not found: {fs_id}")));
        };
        if model.obj_type != SEAF_METADATA_TYPE_DIR as i8 {
            return Err(AppError::NotFound(format!(
                "fs_object {fs_id} is not a directory"
            )));
        }
        let mut data: FsDirData = serde_json::from_str(&model.data)
            .map_err(|e| AppError::internal(format!("fs_object {fs_id}: {e}")))?;
        report.dirs_checked += 1;

        // Structural findings first: they describe the object as stored, before
        // any child is canonicalized, and the repair never acts on them.
        let mut names: HashMap<&str, usize> = HashMap::new();
        for entry in &data.dirents {
            *names.entry(entry.name.as_str()).or_insert(0) += 1;
            let mode = entry.mode & S_IFMT;
            if mode != S_IFDIR_TYPE && mode != S_IFREG_TYPE {
                report.other.push(StructuralFinding {
                    repo_id: self.repo_id.to_string(),
                    path: join(path, &entry.name),
                    kind: "unknown-mode".to_string(),
                    detail: format!("mode {}", entry.mode),
                });
            }
        }
        for (name, count) in names {
            if count > 1 {
                report.other.push(StructuralFinding {
                    repo_id: self.repo_id.to_string(),
                    path: join(path, name),
                    kind: "duplicate-name".to_string(),
                    detail: format!("{count} entries share this name"),
                });
            }
        }

        // The order as stored, with the children left alone: that is what makes
        // this object's id one no client can reproduce.
        let canonical_of_stored = canonical_form(&data);
        if canonical_of_stored != fs_id {
            report.unordered.push(UnorderedDir {
                repo_id: self.repo_id.to_string(),
                path: path.to_string(),
                stored_id: fs_id.to_string(),
                canonical_id: canonical_of_stored,
                dirents: data.dirents.len(),
            });
        }

        for entry in &mut data.dirents {
            if entry.mode & S_IFMT != S_IFDIR_TYPE {
                continue;
            }
            let child_path = join(path, &entry.name);
            // Content addressing rules out a cycle; the depth guard above still
            // bounds a corrupt graph, and the memo bounds the work.
            let child =
                Box::pin(self.canonicalize(&entry.id, &child_path, depth + 1, report)).await;
            match child {
                Ok(new_id) => entry.id = new_id,
                Err(e) => {
                    // Keep the entry as it is: dropping or inventing one would
                    // corrupt a tree the operator may still recover from backup.
                    report.other.push(StructuralFinding {
                        repo_id: self.repo_id.to_string(),
                        path: child_path,
                        kind: "dangling-dirent".to_string(),
                        detail: format!("{}: {e}", entry.id),
                    });
                }
            }
        }

        let new_id = if self.rewrite {
            store_fs_dir_object(self.db, self.repo_id, &data).await?
        } else {
            canonical_form(&data)
        };
        if new_id != fs_id {
            self.rewritten += 1;
        }
        self.memo.insert(fs_id.to_string(), new_id.clone());
        Ok(new_id)
    }
}

/// Append the canonicalizing commit and move HEAD to it, but only while HEAD is
/// still the commit it was computed from (the same compare-and-swap the sync
/// path uses, `DbRepoRepository::test_and_update_head_commit`).
///
/// Returns `Ok(None)` when another writer moved HEAD in between.
async fn append_normalizing_commit(
    db: &DatabaseConnection,
    repo_row: &repo::Model,
    head_commit: &commit::Model,
    new_root: &str,
) -> Result<Option<String>, AppError> {
    let ctime = chrono::Utc::now().timestamp();
    let data = CommitData {
        commit_id: String::new(),
        repo_id: repo_row.id.clone(),
        root_id: new_root.to_string(),
        creator_name: head_commit.creator_name.clone(),
        creator: EMPTY_SHA1.to_string(),
        description: NORMALIZE_DESCRIPTION.to_string(),
        ctime,
        parent_id: Some(head_commit.commit_id.clone()),
        second_parent_id: None,
        repo_name: None,
        repo_desc: None,
        repo_category: None,
        encrypted: None,
        enc_version: None,
        magic: None,
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: repo_row.repo_version,
        conflict: None,
        new_merge: None,
    };
    let commit_id = domain::commit::compute_commit_id(&data);

    commit::Entity::insert(commit::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_row.id.clone()),
        commit_id: Set(commit_id.clone()),
        root_id: Set(new_root.to_string()),
        parent_id: Set(Some(head_commit.commit_id.clone())),
        second_parent_id: Set(None),
        creator_name: Set(head_commit.creator_name.clone()),
        creator: Set(EMPTY_SHA1.to_string()),
        description: Set(NORMALIZE_DESCRIPTION.to_string()),
        ctime: Set(ctime),
        version: Set(repo_row.repo_version as i8),
        new_merge: Set(None),
        conflict: Set(None),
    })
    .exec(db)
    .await?;

    let updated = repo::Entity::update_many()
        .filter(repo::Column::Id.eq(repo_row.id.as_str()))
        .filter(repo::Column::HeadCommitId.eq(head_commit.commit_id.as_str()))
        .set(repo::ActiveModel {
            head_commit_id: Set(Some(commit_id.clone())),
            ..Default::default()
        })
        .exec(db)
        .await?;

    if updated.rows_affected == 1 {
        Ok(Some(commit_id))
    } else {
        Ok(None)
    }
}

/// Id of the same `dirents` serialized in canonical order, without storing.
fn canonical_form(data: &FsDirData) -> String {
    let mut sorted = data.clone();
    domain::fs::sort_dirents_desc(&mut sorted.dirents);
    match domain::fs::compute_dir(&sorted) {
        Some((fs_id, _)) => fs_id,
        None => EMPTY_SHA1.to_string(),
    }
}

async fn find_object(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
) -> Result<Option<fs_object::Model>, AppError> {
    Ok(fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(fs_id))
        .one(db)
        .await?)
}

async fn find_commit(
    db: &DatabaseConnection,
    repo_id: &str,
    commit_id: &str,
) -> Result<Option<commit::Model>, AppError> {
    Ok(commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(commit_id))
        .one(db)
        .await?)
}

/// In-repo path of an entry below `dir_path` (which is `/` or has no trailing
/// slash, as [`DirOrder`] builds them).
fn join(dir_path: &str, name: &str) -> String {
    if dir_path == "/" {
        format!("/{name}")
    } else {
        format!("{dir_path}/{name}")
    }
}
