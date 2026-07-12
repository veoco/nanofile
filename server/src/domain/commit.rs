//! Commit domain logic — serialization and commit_id computation.
//!
//! The `CommitData` type is defined in `base::common`; this module
//! provides the computation functions that were previously methods
//! on that type in `infra::serialization::commit_json`.

use base::common::CommitData;
use sha1::{Digest, Sha1};

/// Serialize commit data to compact JSON.
pub fn to_json(data: &CommitData) -> String {
    serde_json::to_string(data).unwrap()
}

/// Compute the commit_id (SHA1 hash of key commit fields).
///
/// This matches seafile's commit_id computation:
/// sha1(root_id + creator + creator_name + description + ctime_be)
pub fn compute_commit_id(data: &CommitData) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data.root_id.as_bytes());
    hasher.update(data.creator.as_bytes());
    hasher.update(data.creator_name.as_bytes());
    hasher.update(data.description.as_bytes());
    let ctime_be = data.ctime.to_be_bytes();
    hasher.update(ctime_be);
    hex::encode(hasher.finalize())
}
