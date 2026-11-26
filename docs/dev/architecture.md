# Architecture

Nanofile is a Cargo workspace of five crates. Dependencies flow one way:
`base → infra → server`; `migration` and `inference` are consumed by `server`,
and `inference` itself depends only on its two numeric kernels.

| Crate | Role |
|---|---|
| `base` | Pure types, `AppError`, path sanitization, MIME and user-agent helpers. No axum or database dependency unless the `with-axum` / `with-sea-orm` feature enables it. |
| `infra` | Database entities and connections, configuration, crypto, block storage, serialization, rate limiting, the settings catalog and its runtime snapshot, events. |
| `server` | HTTP handlers and pages, services, `AppState`, the Seafile sync protocol, WebDAV, the web UI, file-system engine, sandbox, tasks and the optional tray. |
| `inference` | The embedded models' fixed-plan runtime: operator executor, f32 kernels and the generated plans. No ONNX parser, no server dependency. |
| `migration` | SeaORM migrations. |

## Inside `server`

| Module | Role |
|---|---|
| `handler` | HTTP endpoints: sync API, REST API, uploads, downloads. Thin adapters over services; protocol/page adapters may use the file-system engine directly. |
| `ui` | Askama-rendered web pages and their form handlers, including the admin console. |
| `http` | Presentation support shared by `handler`, `ui` and `webdav`: file-download response construction (Range, ETag, Content-Disposition) and bounded multipart readers. |
| `service` | Business logic and use cases: auth, sync, files, shares, activity, mail, search, notifications, permissions, upload state. Owns the transaction boundary of a use case. |
| `repository` | Database access for accounts, tokens, commits, files and shares: trait interfaces, SeaORM implementations, caching decorators, narrow transactional aggregates, and the activity-log writer. |
| `domain` | Shared authorization model: capabilities, credentials, repository scope, token expiry, commit/fs_id computation. No database or HTTP handles. |
| `fs` | Seafile file-system engine: tree operations, commits, merge, trash, garbage collection, block verification and migration. Also the multi-table transactional operations over commits and file objects. |
| `middleware` | Authentication and capability enforcement, CSP, CSRF, request limits, route audit. |
| `webdav` | WebDAV surface over the same library and file services. |
| `indexer` | Full-text index (Tantivy) and the sandboxed extractors that feed it. |
| `ocr` | PP-OCRv6 tiny inference, the page and text-line orientation classifiers and the DB/CTC post-processing for the full-text index. |
| `sandbox` | The confined worker processes that parse user content. |
| `tasks` | Job registry, queue, executor and run history for background work. Job bodies call services; services submit work through `service::index::IndexScheduler`. |
| `settings` | The layered settings service: saved rows, effective values and the admin save path. The runtime snapshot (`RuntimeConfig`) lives in `infra::settings`. |
| `startup`, `tray` | Auto-start registration and the optional desktop tray. |
| `restart`, `serve`, `app`, `routes` | Process lifecycle, listener, router and application state. |

## Domain

`domain` holds the authorization model rather than the persisted entities:
`Capability`/`CapabilitySet` (the route table classifies method strings),
`Credential` (a session, API key or sync token plus its repository scope),
`RepoScope`, the token-expiry rule, device/session-source values, and the API-key
views (`ApiKeyLookup`, `ApiKeyBinding`) the repository returns. It may refer to
`infra::entity` shapes, but never a database connection or an HTTP type; checks
that need the membership row live in `service::permission`.

## Dependency rules

The layers are directory conventions, not compiler errors. These directions are
wrong and are checked by `tools/check-layering.mjs` in CI:

| Module | May not reference |
|---|---|
| `repository` | `service`, `handler`, `ui`, `webdav`, `middleware`, `tasks`, `sandbox`, `app`, `routes`, `fs` |
| `domain` | `repository`, `service`, `handler`, `ui`, `webdav`, `middleware`, `tasks`, `sandbox`, `app`, `routes` |
| `fs` | `service`, `handler`, `ui`, `webdav`, `middleware`, `sandbox`, `app`, `routes` |
| `sandbox` | `service`, `handler`, `ui`, `webdav`, `middleware`, `tasks`, `app`, `routes` |
| `service` | `handler`, `ui`, `webdav`, `middleware`, `settings`, `app`, `routes` |
| `tasks` | `handler`, `ui`, `webdav`, `middleware`, `app`, `routes` |
| `handler` | `repository`, `sandbox`, `app`, `routes` |
| `middleware` | `handler`, `ui`, `webdav`, `tasks`, `sandbox`, `app`, `routes` |
| `ui` | `handler`, `repository`, `tasks`, `sandbox`, `fs`, `app`, `routes` |
| `webdav` | `handler`, `ui`, `repository`, `app`, `routes` |
| `settings` | `handler`, `ui`, `webdav`, `middleware`, `app`, `routes` |

Directions not listed are allowed (for example `service → repository`,
`handler → service`, `repository → domain`). Four exceptions are deliberate and
carry a reason in the guard script:

- `fs/core/gc.rs` takes a task `JobContext` for progress and cancellation: GC is
  a job.
- `handler` protocol/page adapters (`handler/sync/`,
  `handler/web/{share_view,upload,zip_download,download}.rs`) consume block and
  tree functions directly instead of going through a service.
- `handler/async_batch.rs` and `handler/reindex.rs` submit and read background
  jobs.
- `service/{face_tags,index,mail}` are job bodies and the index scheduler; the
  task system is the service layer's executor.

`node tools/check-layering.mjs --report` prints the known-debt baseline; a new
violation fails the gate even while old ones are being cleaned up. The baseline
is registered per direction and file: `ui → repository` (the admin and page
handlers that read the rows they render), `ui → tasks` (`ui/admintasks.rs`),
`ui → fs` (the file and trash pages), `ui → sandbox` (the settings probe) and
`webdav → repository` (the tree helpers). A file outside the baseline fails, so
the debt cannot grow; `--report` also names entries that no longer match, which
are safe to delete.

## Request flow

A request passes through `middleware` first: the credential is classified as a
session, an API key or a sync token, and the route's required capability is looked
up in one table. It then reaches a `handler`, which calls a `service`; services
use `repository` and `fs` for database and block access. `infra` supplies
configuration, storage and crypto.

The capability table is what makes authorization uniform: a session satisfies
every capability, a key carries only what it was granted, and an unclassified
route is denied rather than allowed. See [Seafile compatibility](/dev/protocol) for
the sync side.

Protocol adapters are the one sanctioned second entry point: the sync protocol,
WebDAV and the anonymous share/upload pages may call the `fs` engine's tree and
block functions directly, because those handlers *are* the protocol. Business
mutations still belong to a service, and a handler never touches a repository.

## Persistence and transactions

`repository` is the CRUD boundary: one trait per aggregate, a SeaORM
implementation, decorators for the auth caches, and the activity-log writer. It is
not the only database path: `fs` also reads and writes entities directly where an
operation spans several tables (tree update + commit + HEAD, archive + delete,
trash restore), and `tasks::setup`/`indexer` use the connection for their own
bookkeeping.

Transaction boundaries follow one rule: **a multi-table invariant is written by a
single repository method that owns a transaction**, not by a service composing
several repository calls. Examples are `history_prune` (commit and file-object
rows of one prune), `provisioning` (library row + owner membership) and
`head_commit` (commit row + HEAD compare-and-swap). There is no generic unit of
work; a flow that must be atomic gets a narrow method instead. Work that is
allowed to lag (activity logging, index scheduling) stays best-effort after the
transaction.

## Background tasks

`tasks` is one registry of jobs, one bounded run store and one executor. Work
enters it two ways:

- `TaskSystem::submit` schedules work somebody will ask about by id. It produces a
  job run that can be polled and, for a cancellable job, stopped.
- `TaskSystem::run_inline` runs request-scoped work under a job's concurrency
  limit, but keeps no record and does not inherit the job's timeout.

A long-lived event listener has no owner, progress or terminal state, so it is a
service rather than a job. A job declares whether every run is recorded
(`EveryRun`) or only runs with something to report (`Notable`); a manual run is
always recorded, and a failure is always recorded.

## Settings

`infra::settings::catalog` is the single catalog of `Config` fields. Every entry
names its `NANOFILE_*` variable, how the value is parsed and when a change takes
effect. The environment layer is applied from that table, so a setting cannot be
added without wiring its variable, and the admin UI cannot advertise a variable
the server does not read. The replaceable snapshot itself (`RuntimeConfig`) lives
in `infra::settings`; `server::settings` builds the layers and persists saves. See
[Configuration](/guide/configure/configuration).

## Build and frontend

`server/build.rs` compiles the Tailwind stylesheet and bundles the frontend entry
points with esbuild. Frontend changes need a `cargo build`; there is no hot
reload. Requirements, the `tray` feature and cross-compilation are in
[Building](/dev/build); the frontend conventions are in [Frontend](/dev/frontend).

## Related

- [Building](/dev/build) — compiling from source: toolchain, frontend assets,
  cross targets.
- [Seafile compatibility](/dev/protocol) — commit ids, `fs_id`, capabilities.
- [Sandbox internals](/dev/sandbox) — the confined worker.
- [Testing](/dev/testing) — the suite and the platform matrix.
- [CI and releases](/dev/ci-releases) — the workflows.
