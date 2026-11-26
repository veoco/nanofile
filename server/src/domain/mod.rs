//! Domain types and policy values shared by the rest of the server.
//!
//! The wire-shaped data lives in `base::common`; this module holds the
//! authorization model (capabilities and credentials), the repository scope,
//! fs_id/commit_id computation and the token-expiry rule. It never holds a
//! database or HTTP handle: checks that need the membership row live in
//! `service::permission`, and the route table matches method strings, not axum
//! types.

pub mod api_key;
pub mod capability;
pub mod commit;
pub mod credential;
pub mod device;
pub mod fs;
pub mod permission;
pub mod repo_path;
pub mod session_source;
pub mod tag;
pub mod token;
