# Nanofile

A wire-compatible Seafile server written in Rust.

## Quick Start

```bash
# Download release binary, or build from source:
cargo build --release

# Configure
cp config.toml .   # edit to suit

# Run
./target/release/nanofile
```

Open `http://localhost:8082`.

## Build

```bash
cargo build --release
```

Optional — install [Tailwind CSS CLI](https://tailwindcss.com/blog/standalone-cli) for a styled web UI:

```bash
curl -sL https://github.com/tailwindlabs/tailwindcss/releases/latest/download/tailwindcss-linux-x64 \
  -o tailwindcss && chmod +x tailwindcss
```

Without it the UI still works, just unstyled.

## Config

Settings are read from `config.toml`. Key fields can also be set via `NANOFILE_*` env vars (e.g. `NANOFILE_DATABASE_URL`, `NANOFILE_SERVER_PORT`).

## Test

```bash
cargo test
```

## License

MIT
