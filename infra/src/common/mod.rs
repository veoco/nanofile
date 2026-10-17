//! Common constants, types and utilities.
//!
//! Domain types are re-exported from `base`, so `crate::common::*` works across all
//! crates; the utility functions live here.

pub use base::common::{
    CommitData, DirEntry, DirEntryData, EMPTY_SHA1, FsDirData, FsFileData, S_IFDIR, S_IFREG,
    SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE,
};

pub mod util;
