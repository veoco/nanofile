//! Domain models and pure computation functions for nanofile.
//!
//! The types live in `base::common`; this module only holds their
//! serialization and fs_id/commit_id computation, with no DB or HTTP access.

pub mod api_key;
pub mod capability;
pub mod commit;
pub mod credential;
pub mod device;
pub mod fs;
pub mod permission;
pub mod repo_path;
pub mod session_source;
