//! Common constants, types, and utility functions.
//!
//! Constants and domain types are re-exported from `base` so that
//! the `crate::common::*` path works consistently across all crates.
//! Utility functions live here in `infra`.

pub use base::common::{
    CommitData, DirEntry, DirEntryData, EMPTY_SHA1, FsDirData, FsFileData, S_IFDIR, S_IFREG,
    SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE,
};

pub mod util;
