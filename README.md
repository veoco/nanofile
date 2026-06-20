# Nanofile

A wire-compatible Seafile server written in Rust.

## Project Structure

Nanofile is organized as a Cargo workspace with three main crates:

| Crate | Role |
|-------|------|
| `base` | Pure base types — `AppError`, `sanitize`, shared constants and types. No HTTP dependency unless `with-axum` feature is enabled. |
| `infra` | Infrastructure — SeaORM entities, crypto, storage backend, serialization, config, permissions, rate limiting. |
| `server` | Application — HTTP handlers, services, repositories, UI, sync protocol, routes. Depends on the two crates above plus axum/tower. |

Dependency direction: `base → infra → server` (compile-time enforced).

## Quick Start

```bash
# Download release binary, or build from source:
cargo build --release -p server

# Configure
cp config.toml .   # edit to suit

# Run
./target/release/server
```

Open `http://localhost:8082`.

## Build

```bash
# Build the server binary
cargo build --release -p server

# Or build the entire workspace
cargo build --release --workspace
```

Optional — install [Tailwind CSS CLI](https://tailwindcss.com/blog/standalone-cli) for a styled web UI:

```bash
curl -sL https://github.com/tailwindlabs/tailwindcss/releases/latest/download/tailwindcss-linux-x64 \
  -o server/tailwindcss && chmod +x server/tailwindcss
```

Without it the UI still works, just unstyled.

## Config

Settings are read from `config.toml` in the working directory. Key fields can also be set via `NANOFILE_*` env vars (e.g. `NANOFILE_DATABASE_URL`, `NANOFILE_SERVER_PORT`).

## Test

```bash
cargo test -p server
```

## License

MIT
