# Seafile compatibility

Nanofile implements the Seafile sync protocol and the APIs the official desktop
and mobile clients use. This page records the compatibility rules that the code
has to preserve. It documents behaviour asserted by code and tests; it does not
claim support for features that are not implemented.

## Commit ids

A commit id is computed exactly like Seafile's `compute_commit_id()`: SHA-1 over,
in order,

1. the root id as 40 hex characters plus a NUL,
2. the creator id as 40 hex characters plus a NUL,
3. the creator name plus a NUL, omitted entirely when the name is empty,
4. the description plus a NUL,
5. the commit time as eight big-endian bytes.

The NUL terminators are part of the hash: every string is fed as a C string
(`strlen + 1`), never as a bare byte slice. Omitting them yields ids no official
client or server would compute for the same logical commit, so a library written
through the web API and the same library written by a desktop client would
disagree on history.

Source: `server/src/domain/commit.rs`; covered by `server/tests/commit_test.rs`.

## Directory order and `fs_id`

A directory object's `fs_id` is the SHA-1 of its serialized form, so the order of
its `dirents` is part of the object's identity.

Official clients always write entries in **descending name order** (upstream
`compare_dirents()` is `strcmp(b, a)`), and they read a stored v1 object back
verbatim. A directory stored in any other order carries an id no client can
reproduce, and the next local write makes the two sides disagree about a directory
neither changed.

`nanofile verify-dir-order` walks every directory reachable from a library's HEAD
and reports objects whose stored order is not canonical. With `--repair` it
appends one ordinary commit per affected library whose root is the canonical form
of the current tree: history is not rewritten, old objects stay readable, and
clients pick the fix up as a normal update. Running it twice changes nothing the
second time.

Source: `server/src/fs/core/dir_order.rs`; covered by `server/tests/dir_order_test.rs`
and `server/tests/fs_id_compat.rs`.

## Received FS objects

A received FS object's id must equal `sha1(uncompressed JSON)` by default:
mismatches are rejected with 400. The official server performs no such check, but
every legitimate repo-v1 client sends a matching id, so strict verification is
safe and prevents a client from storing an object under an id whose content it
does not match.

The policy is `[sync] verify_fs_objects` (`NANOFILE_SYNC_VERIFY_FS_OBJECTS`):
`strict` (default), `log` or `off`. Tree walks are bounded by
`sync.max_tree_depth` and `sync.max_tree_visits` as a cycle and denial-of-service
guard.

## Credentials and capabilities

Every request is classified as one of three credentials:

- **Session** — a browser session or the account-level token a client obtained by
  logging in. It *is* the account, so it satisfies every capability, and it is the
  only credential accepted by the key-management surface.
- **API key** — carries exactly the capabilities it was granted, plus a per-library
  ceiling that can narrow a route's requirement. Granting `file.write` implies
  `file.read`; unknown routes are denied.
- **Sync token** — bound to one library by construction and never accepted by the
  API surfaces; it is refused rather than treated as an account credential.

The route-to-capability table lives in `server/src/domain/capability.rs`; a new
route that is not classified fails closed. `server/tests/capability_coverage_test.rs`
checks that the table covers every route.

## Client quirks preserved

These behaviours are deliberate, because an official client depends on them:

- Activity feed avatar URLs are absolute, because the Android client loads that
  field directly with Glide. A missing author gets a placeholder, so the URL is
  still well formed.
- Repo events use `path="/"` and an empty `name`, matching seahub; nanofile's own
  repo-rename events have no seahub equivalent and keep the new repo name.
- Server-side merges with no conflict use the upstream description
  `Auto merge by system`.

## Tests

The compatibility surface is covered by `commit_test.rs`, `fs_test.rs`,
`fs_id_compat.rs`, `dir_order_test.rs`, `sync_test.rs`, `sync_merge_test.rs`,
`client_compat_test.rs` and `capability_coverage_test.rs` under `server/tests/`.
See [Testing](/dev/testing) for how the suite runs.
