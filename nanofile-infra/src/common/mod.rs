//! Common constants, types, and utility functions.
//!
//! Constants and domain types are re-exported from `nanofile-domain` so that
//! the `crate::common::*` path works consistently across all crates.
//! Utility functions live here in `nanofile-infra`.

pub use nanofile_domain::common::{
    DirEntry, EMPTY_SHA1, S_IFDIR, S_IFREG, SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE,
};

pub mod util;
