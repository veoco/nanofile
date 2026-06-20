/// SHA1 sentinel for empty directories (seafile convention).
/// seafile-server's seaf_dir_new() forces this when entries are NULL;
/// seaf_dir_save() skips persistence for this value.
pub const EMPTY_SHA1: &str = "0000000000000000000000000000000000000000";

/// S_IFREG (0100644) — regular file.
pub const S_IFREG: i32 = 33188;

/// S_IFDIR (040000) — directory.
pub const S_IFDIR: i32 = 16384;

/// SeafMetadataType: file = 1
pub const SEAF_METADATA_TYPE_FILE: i32 = 1;

/// SeafMetadataType: dir = 3
pub const SEAF_METADATA_TYPE_DIR: i32 = 3;
