//! Commit domain logic: serialization and commit_id computation.
//!
//! `CommitData` is defined in `base::common`.

use base::common::CommitData;
use sha1::{Digest, Sha1};

pub fn to_json(data: &CommitData) -> String {
    serde_json::to_string(data).unwrap()
}

/// Compute the commit_id with the wire format's input order:
///
/// 1. `root_id` (40 hex characters) followed by a NUL byte;
/// 2. `creator` (40 hex characters) followed by a NUL byte;
/// 3. `creator_name` followed by a NUL byte, omitted entirely when the name is
///    empty (the format's nullable-field case);
/// 4. `description` followed by a NUL byte;
/// 5. `ctime` as eight big-endian bytes.
///
/// The NUL terminators are part of the hash, not a serialization detail:
/// omitting them yields ids that no official seafile client or server would
/// compute for the same logical commit, so a library written through the web
/// API and the same library written by a desktop client would disagree on
/// history. The format is documented on the compatibility page.
///
/// `creator_name` is nullable upstream and the field is skipped entirely when
/// NULL. nanofile models it as a `String` that is not serialised when empty
/// (see `CommitData::creator_name`), so an empty value is treated as the NULL
/// case here too.
pub fn compute_commit_id(data: &CommitData) -> String {
    compute_commit_id_with_empty_name(data, false)
}

/// Compute the commit id, choosing how an empty `creator_name` participates in the hash.
///
/// [`compute_commit_id`] treats an empty name as upstream's NULL and hashes no bytes for it. A
/// client that sends `creator_name: ""` instead of omitting the field hashed one NUL terminator,
/// and nothing in a JSON `CommitData` distinguishes the two after parsing. Verifying a
/// client-supplied id therefore has to accept either spelling when the name is empty; for a
/// non-empty name both spellings agree, so `include_empty_name` is the only degree of freedom.
pub fn compute_commit_id_with_empty_name(data: &CommitData, include_empty_name: bool) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data.root_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(data.creator.as_bytes());
    hasher.update([0u8]);
    if !data.creator_name.is_empty() || include_empty_name {
        hasher.update(data.creator_name.as_bytes());
        hasher.update([0u8]);
    }
    hasher.update(data.description.as_bytes());
    hasher.update([0u8]);
    hasher.update(data.ctime.to_be_bytes());
    hex::encode(hasher.finalize())
}
