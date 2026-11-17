use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use std::collections::HashSet;
use std::sync::Arc;

use base::common::SEAF_METADATA_TYPE_FILE;
use base::error::AppError;
use infra::entity::fs_object;

#[async_trait]
pub trait FsObjectRepository: Send + Sync {
    async fn find_by_repo_and_fs_id(
        &self,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<Option<fs_object::Model>, AppError>;
    async fn exists_by_repo_and_fs_id(&self, repo_id: &str, fs_id: &str) -> Result<bool, AppError>;
    async fn find_by_repo_and_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<Vec<fs_object::Model>, AppError>;
    /// Project only `fs_id`, without loading the large `data` JSON column; returns the ids that exist.
    async fn find_existing_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<HashSet<String>, AppError>;
    async fn insert_many(&self, models: Vec<fs_object::ActiveModel>) -> Result<(), AppError>;
    /// Whether any FS object in `repo_id` mentions `block_id`. Checked before deleting a block
    /// that an interrupted upload wrote: a concurrent commit of identical content can reference
    /// that very block, and deleting it would corrupt the committed file. The match is a substring
    /// search over the object JSON, so a false positive only leaves an orphan for GC: the safe way.
    async fn references_block(&self, repo_id: &str, block_id: &str) -> Result<bool, AppError>;
    /// Subset of `block_ids` that any stored object of `repo_id` mentions.
    ///
    /// Checked before deleting a block an interrupted upload wrote: a concurrent commit
    /// of identical content can reference that very block, and deleting it would corrupt
    /// the committed file. One pass over the library's object JSON replaces one full
    /// table scan per block, which is quadratic once an abandoned-upload backlog names
    /// thousands of blocks. The match is a 40-hex token scan, so a false positive only
    /// leaves an orphan for GC — the safe direction. An id that is not a 40-character
    /// lowercase hex string is reported as referenced: nothing this store could have
    /// written under it can be ruled out.
    async fn referenced_blocks(
        &self,
        repo_id: &str,
        block_ids: &[String],
    ) -> Result<HashSet<String>, AppError>;
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<fs_object::Model>, AppError>;
    /// Project only `(id, fs_id)` for every object of a repo, without the large `data` JSON column
    /// (used by garbage collection as the pass snapshot); fs_id comes back as its raw 20-byte SHA-1
    /// to avoid allocations.
    async fn find_ids_and_fs_ids_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Vec<(i64, [u8; 20])>, AppError>;
}

pub struct DbFsObjectRepository {
    db: Arc<DatabaseConnection>,
}

impl DbFsObjectRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FsObjectRepository for DbFsObjectRepository {
    async fn find_by_repo_and_fs_id(
        &self,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<Option<fs_object::Model>, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn exists_by_repo_and_fs_id(&self, repo_id: &str, fs_id: &str) -> Result<bool, AppError> {
        // Project only the primary key: the `data` column is a large JSON blob irrelevant to existence.
        let row: Option<(i64,)> = fs_object::Entity::find()
            .select_only()
            .column(fs_object::Column::Id)
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .into_tuple()
            .one(self.db.as_ref())
            .await?;
        Ok(row.is_some())
    }

    async fn find_by_repo_and_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<Vec<fs_object::Model>, AppError> {
        if fs_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        // Chunk the IN list to stay under SQLite's ~999 variable limit; a wide directory can have
        // thousands of direct children, and one unchunked IN query would error.
        for chunk in fs_ids.chunks(500) {
            out.extend(
                fs_object::Entity::find()
                    .filter(fs_object::Column::RepoId.eq(repo_id))
                    .filter(fs_object::Column::FsId.is_in(chunk))
                    .all(self.db.as_ref())
                    .await?,
            );
        }
        Ok(out)
    }

    async fn find_existing_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<HashSet<String>, AppError> {
        let mut out = HashSet::new();
        for chunk in fs_ids.chunks(500) {
            let found: Vec<(String,)> = fs_object::Entity::find()
                .select_only()
                .column(fs_object::Column::FsId)
                .filter(fs_object::Column::RepoId.eq(repo_id))
                .filter(fs_object::Column::FsId.is_in(chunk.to_vec()))
                .into_tuple()
                .all(self.db.as_ref())
                .await?;
            out.extend(found.into_iter().map(|(id,)| id));
        }
        Ok(out)
    }

    async fn insert_many(&self, models: Vec<fs_object::ActiveModel>) -> Result<(), AppError> {
        if models.is_empty() {
            return Ok(());
        }

        // SQLite caps a statement at 32,766 bound variables. This table binds four
        // columns per row (the auto-increment `id` is omitted), so one `INSERT` for
        // a full `recv-fs` pack (several thousand objects when the client fills its
        // 1 MiB pack with tiny objects) would hit the limit. Chunk the rows and keep
        // the batch atomic: a retried sync is idempotent through `ON CONFLICT`, but
        // a partial insert would still make the client resend the rest.
        const INSERT_BATCH: usize = 1_000;
        let txn = self.db.begin().await?;
        let mut models = models.into_iter();
        loop {
            let chunk: Vec<_> = models.by_ref().take(INSERT_BATCH).collect();
            if chunk.is_empty() {
                break;
            }

            // `ON CONFLICT DO NOTHING` makes a re-uploaded object a no-op, and a client re-sending
            // objects an earlier commit already stored (a retried sync, a second commit over the same
            // tree) is legitimate. sea-orm reports "every row conflicted" as `RecordNotInserted`,
            // which is success here.
            match fs_object::Entity::insert_many(chunk)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        fs_object::Column::RepoId,
                        fs_object::Column::FsId,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .exec(&txn)
                .await
            {
                Ok(_) => {}
                Err(sea_orm::DbErr::RecordNotInserted) => {}
                Err(e) => return Err(e.into()),
            }
        }
        txn.commit().await?;
        Ok(())
    }

    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<fs_object::Model>, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn references_block(&self, repo_id: &str, block_id: &str) -> Result<bool, AppError> {
        // Only a content id we could actually have written may authorize a delete; anything else
        // (empty, non-hex, `%`/`_` LIKE metacharacters) reports "referenced" so it cannot remove a block.
        if block_id.is_empty() || !block_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(true);
        }
        let row: Option<(i64,)> = fs_object::Entity::find()
            .select_only()
            .column(fs_object::Column::Id)
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::Data.like(format!("%{block_id}%")))
            .into_tuple()
            .one(self.db.as_ref())
            .await?;
        Ok(row.is_some())
    }

    async fn referenced_blocks(
        &self,
        repo_id: &str,
        block_ids: &[String],
    ) -> Result<HashSet<String>, AppError> {
        if block_ids.is_empty() {
            return Ok(HashSet::new());
        }

        // Decode the candidates once; the scan then compares raw bytes. An id that
        // cannot be a stored block id is kept, exactly as the per-block check did.
        let mut wanted: std::collections::HashMap<[u8; 20], &str> =
            std::collections::HashMap::with_capacity(block_ids.len());
        let mut found: HashSet<String> = HashSet::new();
        for block_id in block_ids {
            match decode_lower_hex_id(block_id) {
                Some(bytes) => {
                    wanted.insert(bytes, block_id.as_str());
                }
                None => {
                    found.insert(block_id.clone());
                }
            }
        }
        if wanted.is_empty() {
            return Ok(found);
        }
        let total = wanted.len() + found.len();

        /// Bound one query's payload; the scan stops after the first match of every
        /// candidate anyway, so a quiet library costs one page.
        const PAGE: u64 = 500;
        let mut last_id = i64::MIN;
        loop {
            let rows: Vec<(i64, String)> = fs_object::Entity::find()
                .select_only()
                .column(fs_object::Column::Id)
                .column(fs_object::Column::Data)
                .filter(fs_object::Column::RepoId.eq(repo_id))
                // Only file objects name blocks; a directory lists fs ids, not blocks.
                .filter(fs_object::Column::ObjType.eq(SEAF_METADATA_TYPE_FILE as i8))
                .filter(fs_object::Column::Id.gt(last_id))
                .order_by_asc(fs_object::Column::Id)
                .limit(PAGE)
                .into_tuple()
                .all(self.db.as_ref())
                .await?;
            if rows.is_empty() {
                break;
            }
            for (id, data) in &rows {
                scan_id_tokens(data, &wanted, &mut found);
                last_id = *id;
            }
            if found.len() >= total {
                break;
            }
        }
        Ok(found)
    }

    async fn find_ids_and_fs_ids_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Vec<(i64, [u8; 20])>, AppError> {
        let rows: Vec<(i64, String)> = fs_object::Entity::find()
            .select_only()
            .column(fs_object::Column::Id)
            .column(fs_object::Column::FsId)
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .into_tuple()
            .all(self.db.as_ref())
            .await?;
        Ok(rows
            .into_iter()
            .map(|(id, fs_id)| {
                let mut buf = [0u8; 20];
                let _ = hex::decode_to_slice(&fs_id, &mut buf);
                (id, buf)
            })
            .collect())
    }
}

/// Length of a SHA-1 id in hex, which is how every block and object id is stored.
const ID_HEX_LEN: usize = 40;

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn decode_lower_hex_id(id: &str) -> Option<[u8; 20]> {
    if id.len() != ID_HEX_LEN || !id.bytes().all(is_lower_hex) {
        return None;
    }
    let mut buf = [0u8; 20];
    hex::decode_to_slice(id, &mut buf).ok()?;
    Some(buf)
}

/// Visit every 40-character lowercase-hex run in `data` and record the wanted ones.
///
/// Stored ids sit between quotes, so a real id is exactly one run; a longer run (some
/// other hex value) or a shorter one is ignored. A match means the object literally
/// contains the block id, and a false positive would only keep a block.
fn scan_id_tokens(
    data: &str,
    wanted: &std::collections::HashMap<[u8; 20], &str>,
    found: &mut HashSet<String>,
) {
    let bytes = data.as_bytes();
    let mut start = 0usize;
    let mut len = 0usize;
    for (i, byte) in bytes.iter().enumerate() {
        if is_lower_hex(*byte) {
            if len == 0 {
                start = i;
            }
            len += 1;
        } else {
            record_id_token(bytes, start, len, wanted, found);
            len = 0;
        }
    }
    record_id_token(bytes, start, len, wanted, found);
}

fn record_id_token(
    bytes: &[u8],
    start: usize,
    len: usize,
    wanted: &std::collections::HashMap<[u8; 20], &str>,
    found: &mut HashSet<String>,
) {
    if len != ID_HEX_LEN {
        return;
    }
    let mut buf = [0u8; 20];
    if hex::decode_to_slice(&bytes[start..start + ID_HEX_LEN], &mut buf).is_err() {
        return;
    }
    if let Some(original) = wanted.get(&buf) {
        found.insert((*original).to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};

    async fn repo() -> DbFsObjectRepository {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        Migrator::up(&db, None).await.expect("migrate");
        // The objects are seeded without their library parent; the scan under test
        // reads the rows and the foreign key has nothing to add.
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "PRAGMA foreign_keys = OFF",
        ))
        .await
        .expect("disable foreign keys");
        DbFsObjectRepository::new(Arc::new(db))
    }

    async fn insert(db: &DatabaseConnection, id: i64, repo_id: &str, data: &str) {
        let fs_id = format!("{id:040x}");
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!("INSERT INTO fs_objects (id, repo_id, fs_id, obj_type, data) VALUES ({id}, '{repo_id}', '{fs_id}', 1, '{data}')"),
        ))
        .await
        .expect("insert fs object");
    }

    /// The scan is scoped to one library, finds ids inside the stored JSON, and keeps
    /// an id it could not have written (the conservative direction).
    #[tokio::test]
    async fn referenced_blocks_is_scoped_and_conservative() {
        let repo = repo().await;
        let db = repo.db.as_ref();
        let hit = "a1".repeat(20);
        let miss = "b2".repeat(20);
        insert(
            db,
            1,
            "r1",
            &format!(r#"{{"block_ids":["{hit}"],"size":1,"type":1,"version":1}}"#),
        )
        .await;
        insert(
            db,
            2,
            "r2",
            &format!(r#"{{"block_ids":["{miss}"],"size":1,"type":1,"version":1}}"#),
        )
        .await;

        let found = repo
            .referenced_blocks("r1", &[hit.clone(), miss.clone(), "not-an-id".to_string()])
            .await
            .expect("scan");
        assert!(found.contains(&hit));
        assert!(
            !found.contains(&miss),
            "another library's object must not protect the block"
        );
        assert!(
            found.contains("not-an-id"),
            "an id that cannot be stored is reported as referenced"
        );
    }

    /// The scan pages through the library, so a match past the first page is still seen.
    #[tokio::test]
    async fn referenced_blocks_pages_through_a_large_library() {
        let repo = repo().await;
        let db = repo.db.as_ref();
        let hit = "c3".repeat(20);
        for id in 1..=501 {
            let data = if id == 501 {
                format!(r#"{{"block_ids":["{hit}"]}}"#)
            } else {
                r#"{"block_ids":[]}"#.to_string()
            };
            insert(db, id, "r1", &data).await;
        }

        let found = repo
            .referenced_blocks("r1", std::slice::from_ref(&hit))
            .await
            .expect("scan");
        assert!(found.contains(&hit), "the second page must be reached");
    }
}
