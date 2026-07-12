//! # domain
//!
//! Domain models and computation functions for nanofile.
//!
//! The type definitions (FsDirData, FsFileData, DirEntryData, CommitData)
//! live in `base::common` so they can be shared with the infra crate.
//! This module provides the computation functions that operate on those
//! types — serialization to compact JSON, SHA1 fs_id computation, etc.
//!
//! This module intentionally avoids infrastructure concerns (DB access,
//! HTTP, etc.) — it's pure domain logic.

pub mod commit;
pub mod fs;
