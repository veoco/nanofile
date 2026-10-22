# Architecture

Nanofile is a Cargo workspace of four crates. Dependencies flow one way:
`base → infra → server`; `migration` is consumed by `server`.

| Crate | Role |
|---|---|
| `base` | Pure domain types, `AppError`, path sanitization. No axum or database dependency unless the `with-axum` / `with-sea-orm` feature enables it. |
| `infra` | Database entities and connections, configuration, crypto, block storage, serialization, permissions, rate limiting, search indexing, the settings catalog and its events. |
| `server` | HTTP handlers and routes, services, `AppState`, the Seafile sync protocol, WebDAV, the web UI, sandbox, tasks and the optional tray. |
| `migration` | SeaORM migrations. |

## Inside `server`

| Module | Role |
|---|---|
| `handler` | HTTP endpoints: sync API, REST API, web pages, shares, uploads. |
| `service` | Business logic: auth, sync, files, shares, activity, mail, search, notifications. |
| `repository` | Database access for accounts, tokens, commits, files and shares. |
| `domain` | Shared domain types: credentials, capabilities, commits, file-system objects. |
| `fs` | Seafile file-system objects: tree operations, commits, merge, trash, garbage collection, block verification and migration. |
| `ui` | Askama-rendered web pages, including the admin console. |
| `middleware` | Authentication and capabilities, CSP, CSRF, request limits. |
| `webdav` | WebDAV surface over the same library and file services. |
| `indexer` | Full-text index (Tantivy) and the sandboxed extractors that feed it. |
| `sandbox` | The confined worker processes that parse user content. |
| `tasks` | Job registry, queue, executor and run history for background work. |
| `settings` | Runtime configuration, settings layering and the admin settings service. |
| `startup`, `tray` | Auto-start registration and the optional desktop tray. |
| `restart`, `serve`, `app`, `routes` | Process lifecycle, listener, router and application state. |

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

## Background tasks

`tasks` is one registry of jobs, one bounded run store and one executor. Work
enters it two ways:

- `TaskSystem::submit` schedules work somebody will ask about by id. It produces a
  job run that can be polled and, for a cancellable job, stopped.
- `TaskSystem::run_inline` runs request-scoped work under the same concurrency
  limits and timeouts but keeps no record.

A long-lived event listener has no owner, progress or terminal state, so it is a
service rather than a job. A job declares whether every run is recorded
(`EveryRun`) or only runs with something to report (`Notable`); a manual run is
always recorded, and a failure is always recorded.

## Settings

`infra::settings::catalog` is the single catalog of `Config` fields. Every entry
names its `NANOFILE_*` variable, how the value is parsed and when a change takes
effect. The environment layer is applied from that table, so a setting cannot be
added without wiring its variable, and the admin UI cannot advertise a variable
the server does not read. See [Configuration](/guide/configuration).

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
