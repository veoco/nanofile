pub mod commit_json;
pub mod fs_json;
pub mod pack_fs;

/// Seafile dirent mode constants (from stat.h).
/// S_IFREG (0100644) — regular file.
pub const S_IFREG: i32 = 33188;
/// S_IFDIR (040000) — directory.
pub const S_IFDIR: i32 = 16384;
