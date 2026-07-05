//! Full-text search indexing using Tantivy + jieba tokenizer.
//!
//! Creates/opens a Tantivy index directory on disk. Registers the `jieba`
//! Chinese tokenizer (via tantivy-jieba) for content fields so both CJK and
//! Latin text are searchable.
//!
//! # Text Detection
//!
//! `is_indexable_text()` determines whether a file should be indexed:
//! 1. Known text-file extensions (`.txt .rs .py .md …`)
//! 2. Content sniffing — first 1024 bytes contain no NUL bytes and are
//!    valid UTF-8.
//!
//! # Background Commit
//!
//! A tokio task commits the Tantivy writer every 30 seconds so uncommitted
//! documents don't stall on a crash.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tantivy::Index;
use tantivy::ReloadPolicy;
use tantivy::TantivyDocument;
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::schema::*;
use tantivy::tokenizer::TextAnalyzer;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::storage::DynBlockStorage;

// ── Schema field names ────────────────────────────────────────────────
const FIELD_REPO_ID: &str = "repo_id";
const FIELD_FULLPATH: &str = "fullpath";
const FIELD_FILENAME: &str = "filename";
const FIELD_CONTENT: &str = "content";

/// File extensions considered indexable plain text.
const TEXT_EXTENSIONS: &[&str] = &[
    // Code
    "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "rb", "php", "c", "cpp", "h", "hpp", "cs",
    "swift", "kt", "scala", "dart", "lua", "pl", "pm", "r", "m", "mm", "clj", "cljs", "coffee",
    "groovy", "erl", "hrl", "fs", "fsx", "hs", "lhs", "nim", "zig", "v", "vhdl", "asm", "s", "awk",
    "cbl", "cc", "cfc", "cfm", "cob", "cpy", "d", "e", "el", "ex", "exs", "f", "f90", "f95", "for",
    "frag", "fsh", "geo", "glsl", "gml", "gql", "graphql", "gyp", "hbs", "hxx", "ino", "ipp", "j",
    "jl", "kt", "kts", "lagda", "lisp", "ll", "lm", "lpr", "ls", "m4", "mak", "ml", "mli", "mll",
    "mly", "mo", "mod", "ms", "mt", "nix", "njk", "nqp", "ox", "oxh", "oxo", "p6", "p7s", "pas",
    "pck", "pd", "pdd", "pkh", "pig", "pl6", "pls", "pm6", "pod", "pod6", "pp", "prc", "prefs",
    "pro", "proto", "ps", "ps1", "psd1", "psm1", "pt", "purs", "pxd", "pxi", "pyx", "qbs", "qml",
    "r2", "r3", "rake", "rbw", "rbx", "rhtml", "rkt", "rmd", "rno", "roff", "rpy", "rq", "rsx",
    "ru", "sage", "sas", "sass", "sc", "scad", "scm", "scss", "sed", "sfd", "sh", "sjs", "sls",
    "sml", "sps", "sqf", "sr", "ss", "st", "styl", "sv", "t", "tcc", "tcl", "tex", "textile",
    "tla", "tlx", "tpl", "tpp", "tst", "ttl", "twig", "uc", "udf", "vala", "vbs", "vhd", "vim",
    "vm", "vsh", "w", "wast", "wat", "webidl", "xib", "xl", "xqy", "xquery", "xsd", "xsl", "xslt",
    "xul", "yang", "yaws", "yxx", "yy", "zep", // Scripts / config
    "sh", "bash", "zsh", "fish", "bat", "cmd", "ps1", "gradle", "cmake", "make", "mk",
    // Web
    "html", "htm", "xhtml", "css", "scss", "less", "sass", "vue", "svelte", "ejs", "erb", "hbs",
    "mustache", "haml", "slim", "jade", "pug", // Data / markup
    "json", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf", "env", "csv", "tsv", "sql",
    "graphql", // Docs
    "txt", "text", "md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "rst", "rtf", "tex", "bib",
    "log", "org", "pod", "wiki", "creole", "rest", "asc", "adoc", "asciidoc", "docbook",
    // Other
    "diff", "patch", "po", "pot", "spec",
];

/// Full-text indexer wrapping Tantivy.
#[derive(Clone)]
pub struct TextIndexer {
    index: Index,
    writer: Arc<Mutex<tantivy::IndexWriter<TantivyDocument>>>,
    schema: Schema,
    reader: tantivy::IndexReader,
}

impl TextIndexer {
    /// Create or open a Tantivy index at `index_dir`.
    ///
    /// Registers the `jieba` Chinese tokenizer. Returns an error if the
    /// directory cannot be created or the index is incompatible.
    pub fn new(index_dir: &Path) -> Result<Self, AppError> {
        std::fs::create_dir_all(index_dir)
            .map_err(|e| AppError::internal(format!("create index dir: {e}")))?;

        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field(FIELD_REPO_ID, STRING | STORED);
        schema_builder.add_text_field(FIELD_FULLPATH, STRING | STORED);
        schema_builder.add_text_field(FIELD_FILENAME, TEXT | STORED);
        schema_builder.add_text_field(
            FIELD_CONTENT,
            TextOptions::default().set_stored().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("jieba")
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            ),
        );
        let schema = schema_builder.build();

        let index = match Index::create_in_dir(index_dir, schema.clone()) {
            Ok(index) => index,
            Err(_) => {
                // Index directory exists but may have incompatible schema
                // (e.g. from a previous version). Delete and recreate.
                tracing::info!("Rebuilding full-text index at {:?}", index_dir);
                std::fs::remove_dir_all(index_dir)
                    .map_err(|e| AppError::internal(format!("remove old index dir: {e}")))?;
                std::fs::create_dir_all(index_dir)
                    .map_err(|e| AppError::internal(format!("create index dir: {e}")))?;
                Index::create_in_dir(index_dir, schema.clone())
                    .map_err(|e| AppError::internal(format!("create tantivy index: {e}")))?
            }
        };

        // Register the jieba Chinese tokenizer for content fields.
        // LowerCaser ensures case-insensitive search for non-CJK text.
        index.tokenizers().register(
            "jieba",
            TextAnalyzer::builder(tantivy_jieba::JiebaTokenizer::new())
                .filter(tantivy::tokenizer::LowerCaser)
                .build(),
        );

        let writer: tantivy::IndexWriter<TantivyDocument> = index
            .writer(50_000_000) // 50 MB memory budget
            .map_err(|e| AppError::internal(format!("create tantivy writer: {e}")))?;

        // Use Manual reload policy — callers must commit() explicitly and the reader
        // picks up the new version on the next search() call via searcher.reload().
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| AppError::internal(format!("create tantivy reader: {e}")))?;

        let writer_arc = Arc::new(Mutex::new(writer));

        Ok(Self {
            index: index.clone(),
            writer: writer_arc,
            schema,
            reader,
        })
    }

    /// Index a file's text content.
    ///
    /// * `repo_id` — repository UUID.
    /// * `fullpath` — absolute path within the repo (e.g. `/dir/file.txt`).
    /// * `filename` — file name only (e.g. `file.txt`).
    /// * `content` — text content to index.
    ///
    /// If a document with the same `(repo_id, fullpath)` already exists, it
    /// is replaced (delete + add).
    pub fn index_file(
        &self,
        repo_id: &str,
        fullpath: &str,
        filename: &str,
        content: &str,
    ) -> Result<(), AppError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| AppError::internal(format!("indexer mutex poisoned: {e}")))?;

        // Delete any existing document with this (repo_id, fullpath) pair.
        self.delete_docs_inner(&mut writer, repo_id, fullpath)?;

        let repo_id_field = self
            .schema
            .get_field(FIELD_REPO_ID)
            .expect("repo_id field defined");
        let fullpath_field = self
            .schema
            .get_field(FIELD_FULLPATH)
            .expect("fullpath field defined");
        let filename_field = self
            .schema
            .get_field(FIELD_FILENAME)
            .expect("filename field defined");
        let content_field = self
            .schema
            .get_field(FIELD_CONTENT)
            .expect("content field defined");

        let doc = doc!(
            repo_id_field => repo_id,
            fullpath_field => fullpath,
            filename_field => filename,
            content_field => content,
        );

        writer
            .add_document(doc)
            .map_err(|e| AppError::internal(format!("index add doc: {e}")))?;

        // No automatic commit here — the background committer persists pending
        // documents periodically.  Call commit() manually when immediate
        // durability is required (e.g. before server shutdown).
        Ok(())
    }

    /// Delete the indexed document for `(repo_id, fullpath)`.
    pub fn delete_file(&self, repo_id: &str, fullpath: &str) -> Result<(), AppError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| AppError::internal(format!("indexer mutex poisoned: {e}")))?;
        self.delete_docs_inner(&mut writer, repo_id, fullpath)?;
        // No automatic commit — the background committer handles persistence.
        Ok(())
    }

    /// Explicitly commit all pending index operations.
    /// Useful in tests and before server shutdown.
    pub fn commit(&self) -> Result<(), AppError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| AppError::internal(format!("indexer mutex poisoned: {e}")))?;
        writer
            .commit()
            .map_err(|e| AppError::internal(format!("index commit: {e}")))?;
        Ok(())
    }

    /// Explicitly reload the reader to pick up newly committed documents.
    /// Returns the number of segments affected.
    pub fn reload(&self) -> Result<(), AppError> {
        self.reader
            .reload()
            .map_err(|e| AppError::internal(format!("index reload: {e}")))
    }

    /// Spawn a background task that commits the Tantivy writer periodically.
    ///
    /// The task commits every 30 seconds so uncommitted documents don't
    /// accumulate indefinitely.  On shutdown the caller should also call
    /// `commit()` explicitly.
    ///
    /// The task exits when `token` is cancelled.
    pub fn spawn_background_committer(&self, token: CancellationToken) {
        let writer = self.writer.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Ok(mut w) = writer.lock()
                            && let Err(e) = w.commit()
                        {
                            tracing::warn!("Background index commit failed: {e}");
                        }
                    }
                    _ = token.cancelled() => {
                        tracing::info!("Background index committer shutting down");
                        break;
                    }
                }
            }
        });
    }

    /// Re-index a file by reading its content from block storage.
    ///
    /// Used after rename/move operations (the content is still in storage but
    /// the path has changed), and by the re-index API for backfilling existing
    /// files.
    pub async fn reindex_file(
        &self,
        db: &DatabaseConnection,
        repo_id: &str,
        fullpath: &str,
        block_store: &DynBlockStorage,
    ) -> Result<(), AppError> {
        let filename = fullpath
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(fullpath);

        // Read file content from block storage.
        let data = match crate::repo::download::Downloader::download_file(
            db,
            repo_id,
            fullpath,
            block_store,
            None,
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("reindex_file: failed to read {fullpath}: {e}");
                return Err(AppError::internal(format!(
                    "reindex_file: read {fullpath}: {e}"
                )));
            }
        };

        if is_indexable_text(filename, &data) {
            let content = String::from_utf8_lossy(&data);
            self.index_file(repo_id, fullpath, filename, &content)?;
        } else {
            // Not indexable — clean up any old index entry.
            self.delete_file(repo_id, fullpath)?;
        }

        Ok(())
    }

    /// Search the full-text index.
    ///
    /// Uses Tantivy's standard QueryParser for exact term matching.
    /// Case-insensitivity is handled by the LowerCaser filter on the
    /// jieba tokenizer. For substring/prefix matching on filenames, the
    /// caller should also run the filename search fallback (FS tree
    /// traversal with `strcasestr`).
    ///
    /// When `filename_only` is true, only the filename field is searched.
    /// This is used by the Web UI's filename search to avoid the expensive
    /// FS tree walk when an index is available.
    ///
    /// Returns a list of `(repo_id, fullpath)` tuples matching the keyword.
    /// Results are limited by `limit` and offset by `offset`.
    /// If `repo_ids` is non-empty, only results from those repos are returned.
    pub fn search(
        &self,
        keyword: &str,
        repo_ids: &[String],
        limit: usize,
        offset: usize,
        filename_only: bool,
    ) -> Result<Vec<(String, String)>, AppError> {
        if keyword.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Commit any pending documents first so the reader can pick them up.
        // This ensures search always returns the latest content without
        // requiring callers to explicitly commit after every index operation.
        if let Err(e) = self.commit() {
            tracing::warn!("search: commit before search failed: {e}");
        } else if let Err(e) = self.reader.reload() {
            tracing::warn!("search: reload after commit failed: {e}");
        }
        let reader = self.reader.clone();
        let searcher = reader.searcher();

        let filename_field = self
            .schema
            .get_field(FIELD_FILENAME)
            .expect("filename field defined");
        let content_field = self
            .schema
            .get_field(FIELD_CONTENT)
            .expect("content field defined");
        let repo_id_field = self
            .schema
            .get_field(FIELD_REPO_ID)
            .expect("repo_id field defined");
        let fullpath_field = self
            .schema
            .get_field(FIELD_FULLPATH)
            .expect("fullpath field defined");

        // Build a BooleanQuery that combines:
        // 1. Exact term matching via standard QueryParser.
        // 2. Prefix matching via RegexQuery (pattern `term.*` without `^`
        //    anchor — Tantivy-fst's automaton matches full term strings).
        //    This makes "case" match "caseend", "casetest", etc.
        // 3. Optional repo_id filter via TermQuery (MUST), pushed down so
        //    Tantivy only scores and returns docs for matching repos.
        // The LowerCaser filter on the jieba tokenizer ensures case-insensitivity.
        use tantivy::query::{BooleanQuery, Occur, QueryParser, RegexQuery, TermQuery};
        use tantivy::schema::IndexRecordOption;

        let query_fields: Vec<Field> = if filename_only {
            vec![filename_field]
        } else {
            vec![filename_field, content_field]
        };
        let query_parser = QueryParser::for_index(&self.index, query_fields);
        let exact_query = query_parser
            .parse_query(keyword)
            .map_err(|e| AppError::internal(format!("parse query: {e}")))?;

        let mut subqueries: Vec<(Occur, Box<dyn tantivy::query::Query>)> =
            vec![(Occur::Should, exact_query)];

        for term in keyword.split_whitespace() {
            // Escape regex special chars in the user's search term.
            let mut escaped = String::with_capacity(term.len() + 4);
            for c in term.chars() {
                match c {
                    '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}'
                    | '^' | '$' | '#' => {
                        escaped.push('\\');
                        escaped.push(c);
                    }
                    c => escaped.push(c),
                }
            }
            // `hello.*` matches any term starting with "hello".
            // No `^` anchor — Tantivy-fst's regex automaton matches the
            // full term string by design.
            let pattern = format!("{escaped}.*");

            let regex_fields: &[Field] = if filename_only {
                &[filename_field]
            } else {
                &[filename_field, content_field]
            };
            for &field in regex_fields {
                if let Ok(regex_q) = RegexQuery::from_pattern(&pattern, field) {
                    subqueries.push((Occur::Should, Box::new(regex_q)));
                }
            }
        }

        // Build the final query.
        // When repo_ids is non-empty, wrap everything in a top-level Must:
        //   Must(text_or_prefix_match) AND Must(repo_id filter)
        let query: Box<dyn tantivy::query::Query> = if !repo_ids.is_empty() {
            let text_query: Box<dyn tantivy::query::Query> = if subqueries.len() == 1 {
                subqueries
                    .into_iter()
                    .next()
                    .map(|(_, q)| q)
                    .expect("non-empty subqueries")
            } else {
                Box::new(BooleanQuery::new(subqueries))
            };

            let repo_query: Box<dyn tantivy::query::Query> = if repo_ids.len() == 1 {
                Box::new(TermQuery::new(
                    tantivy::Term::from_field_text(repo_id_field, &repo_ids[0]),
                    IndexRecordOption::Basic,
                ))
            } else {
                let repo_subqueries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = repo_ids
                    .iter()
                    .map(|rid| {
                        let tq: Box<dyn tantivy::query::Query> = Box::new(TermQuery::new(
                            tantivy::Term::from_field_text(repo_id_field, rid),
                            IndexRecordOption::Basic,
                        ));
                        (Occur::Should, tq)
                    })
                    .collect();
                Box::new(BooleanQuery::new(repo_subqueries))
            };

            // Top-level AND: text match AND repo_id filter.
            let top = vec![(Occur::Must, text_query), (Occur::Must, repo_query)];
            Box::new(BooleanQuery::new(top))
        } else if subqueries.len() == 1 {
            subqueries
                .into_iter()
                .next()
                .map(|(_, q)| q)
                .expect("non-empty subqueries")
        } else {
            Box::new(BooleanQuery::new(subqueries))
        };

        // Collect enough results for offset + limit.
        let top_docs = searcher
            .search(
                &query,
                &TopDocs::with_limit(limit + offset).order_by_score(),
            )
            .map_err(|e| AppError::internal(format!("search: {e}")))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs.into_iter().skip(offset) {
            let doc = searcher
                .doc::<TantivyDocument>(doc_address)
                .map_err(|e| AppError::internal(format!("retrieve doc: {e}")))?;

            let repo_id = doc
                .get_first(repo_id_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let fullpath = doc
                .get_first(fullpath_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            results.push((repo_id, fullpath));

            // Stop when we've collected enough.
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Delete all documents matching `(repo_id, fullpath)`.
    fn delete_docs_inner(
        &self,
        writer: &mut tantivy::IndexWriter<TantivyDocument>,
        repo_id: &str,
        fullpath: &str,
    ) -> Result<(), AppError> {
        use tantivy::query::{BooleanQuery, Occur, TermQuery};
        use tantivy::schema::IndexRecordOption;

        let repo_id_field = self
            .schema
            .get_field(FIELD_REPO_ID)
            .expect("repo_id field defined");
        let fullpath_field = self
            .schema
            .get_field(FIELD_FULLPATH)
            .expect("fullpath field defined");

        // Build a boolean query with ALL terms marked as Must (AND).
        let subqueries = vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    tantivy::Term::from_field_text(repo_id_field, repo_id),
                    IndexRecordOption::WithFreqs,
                )) as Box<dyn tantivy::query::Query>,
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    tantivy::Term::from_field_text(fullpath_field, fullpath),
                    IndexRecordOption::WithFreqs,
                )) as Box<dyn tantivy::query::Query>,
            ),
        ];
        let query = BooleanQuery::new(subqueries);
        writer
            .delete_query(Box::new(query))
            .map_err(|e| AppError::internal(format!("delete query: {e}")))?;

        Ok(())
    }
}

/// Determine whether a file should be indexed as plain text.
///
/// Uses a two-phase check:
/// 1. Extension whitelist — fast, file-specific.
/// 2. Content sniffing — for unknown extensions, check the first 1024 bytes
///    contain no NUL bytes and are valid UTF-8.
pub fn is_indexable_text(filename: &str, data: &[u8]) -> bool {
    // Extract extension, lowercased.
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();

    // Phase 1: known text extensions.
    if TEXT_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }

    // Phase 2: content sniffing for unknown extensions / no extension.
    text_content_sniff(data)
}

/// Sniff whether `data` looks like plain text by checking the first 1024
/// bytes for NUL bytes and UTF-8 validity.
fn text_content_sniff(data: &[u8]) -> bool {
    let head = if data.len() > 1024 {
        &data[..1024]
    } else {
        data
    };

    // NUL byte → binary.
    if head.contains(&0) {
        return false;
    }

    // Must be valid UTF-8.
    std::str::from_utf8(head).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_indexable_text_known_extensions() {
        assert!(is_indexable_text("hello.rs", b"fn main() {}"));
        assert!(is_indexable_text("main.py", b"print('hello')"));
        assert!(is_indexable_text("readme.md", b"# Title"));
        assert!(is_indexable_text("config.toml", b"[server]"));
        assert!(is_indexable_text("index.html", b"<html>"));
        assert!(is_indexable_text("style.css", b"body {}"));
        assert!(is_indexable_text("data.json", b"{}"));
        assert!(is_indexable_text("script.sh", b"#!/bin/bash"));
        assert!(is_indexable_text("README.txt", b"plain text"));
    }

    #[test]
    fn test_is_indexable_text_binary() {
        assert!(!is_indexable_text("image.png", b"\x89PNG\r\n\x1a\n"));
        // PDF starts with %PDF-1.4 which looks like text, so it's caught by extension check.
        // Binary detection via content: has null bytes.
        assert!(!is_indexable_text("binary.bin", &[0, 1, 2, 3, 4, 5]));
        assert!(!is_indexable_text("data.raw", b"text\x00binary"));
    }

    #[test]
    fn test_is_indexable_text_sniffing() {
        // File with no extension but valid text content.
        assert!(is_indexable_text("README", b"Hello World"));
        // File with no extension and binary content.
        assert!(!is_indexable_text("data", &[0x00, 0x01, 0x02]));
    }

    #[test]
    fn test_is_indexable_text_empty() {
        // Empty file should be valid.
        assert!(is_indexable_text("notes.txt", b""));
    }

    #[tokio::test]
    async fn test_index_and_search() -> Result<(), AppError> {
        let dir = tempfile::tempdir().unwrap();
        let indexer = TextIndexer::new(dir.path())?;

        indexer.index_file(
            "repo-1",
            "/hello.txt",
            "hello.txt",
            "Hello World, this is a test file",
        )?;

        // Commit so the reader can pick up the new document.
        indexer.commit()?;

        let results = indexer.search("hello", &[], 10, 0, false)?;
        assert_eq!(
            results.len(),
            1,
            "should find 'hello' in filename, got {:?}",
            results
        );
        assert_eq!(results[0], ("repo-1".to_string(), "/hello.txt".to_string()));

        // Search for content (not filename).
        let results = indexer.search("test file", &[], 10, 0, false)?;
        assert_eq!(results.len(), 1, "should find 'test file' in content");
        assert_eq!(results[0], ("repo-1".to_string(), "/hello.txt".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_search_filter_by_repo() -> Result<(), AppError> {
        let dir = tempfile::tempdir().unwrap();
        let indexer = TextIndexer::new(dir.path())?;

        indexer.index_file("repo-1", "/a.txt", "a.txt", "alpha")?;
        indexer.index_file("repo-2", "/b.txt", "b.txt", "beta")?;

        // Commit so the reader can pick up the new documents.
        indexer.commit()?;

        let results = indexer.search("alpha", &["repo-1".to_string()], 10, 0, false)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "repo-1");

        // Scoped to wrong repo — no results.
        let results = indexer.search("alpha", &["repo-2".to_string()], 10, 0, false)?;
        assert_eq!(results.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_file() -> Result<(), AppError> {
        let dir = tempfile::tempdir().unwrap();
        let indexer = TextIndexer::new(dir.path())?;

        indexer.index_file("repo-1", "/hello.txt", "hello.txt", "Hello World")?;
        indexer.commit()?;
        assert_eq!(indexer.search("hello", &[], 10, 0, false)?.len(), 1);

        indexer.delete_file("repo-1", "/hello.txt")?;
        indexer.commit()?;

        assert_eq!(indexer.search("hello", &[], 10, 0, false)?.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_index_update_replaces() -> Result<(), AppError> {
        let dir = tempfile::tempdir().unwrap();
        let indexer = TextIndexer::new(dir.path())?;

        indexer.index_file("repo-1", "/file.txt", "file.txt", "old content")?;
        indexer.commit()?;

        let results = indexer.search("old", &[], 10, 0, false)?;
        assert_eq!(results.len(), 1);

        // Index same path with new content.
        indexer.index_file("repo-1", "/file.txt", "file.txt", "new content")?;
        indexer.commit()?;

        // Old content should no longer match.
        assert_eq!(indexer.search("old", &[], 10, 0, false)?.len(), 0);
        // New content should match.
        assert_eq!(indexer.search("new", &[], 10, 0, false)?.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_pagination() -> Result<(), AppError> {
        let dir = tempfile::tempdir().unwrap();
        let indexer = TextIndexer::new(dir.path())?;

        for i in 0..10 {
            let name = format!("file-{}.txt", i);
            let content = format!("content number {}", i);
            indexer.index_file("repo-1", &format!("/{}", name), &name, &content)?;
        }

        indexer.commit()?;

        // First page: limit=3, offset=0
        let results = indexer.search("content", &[], 3, 0, false)?;
        assert_eq!(results.len(), 3, "first page should have 3");

        // Second page: limit=3, offset=3
        let results = indexer.search("content", &[], 3, 3, false)?;
        assert_eq!(results.len(), 3, "second page should have 3");

        // Last page: limit=3, offset=9
        let results = indexer.search("content", &[], 3, 9, false)?;
        assert_eq!(results.len(), 1, "last page should have 1");

        Ok(())
    }
}
