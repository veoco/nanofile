# Frontend

The web interface is server-rendered with Askama templates and progressively
enhanced with modular JavaScript. Frontend changes require a `cargo build`; there
is no hot reload.

## Templates and styles

- Templates live in `server/templates/` and are rendered by `server/src/ui/`.
- Tailwind input is `server/static/css/input.css`; the generated stylesheet is
  `server/static/css/app.css`.
- The favicon and the generated tray icon come from
  `server/static/img/favicon.svg`, rasterized at compile time.

## JavaScript

- Source modules live in `server/frontend/` (`core/`, `browser/`, `pages/`).
- Entry points are `server/frontend/entries/*.js`.
- `server/build.rs` bundles each entry with esbuild into
  `server/static/js/*.bundle.js` and embeds the result in the binary.
- esbuild is required. Tailwind is optional; without it the build falls back to a
  placeholder stylesheet and the UI is unstyled.

The bundle is compiled into the binary, so editing a frontend file does nothing
until `cargo build` runs.

The auth pages also load a separate `pow.bundle.js`; it does nothing unless the
server rendered a proof-of-work element. It is the browser half of the self-hosted challenge: it
solves `SHA-256(seed || nonce)` in time-bounded chunks, then fills the hidden
nonce. Its SHA-256 and solver live in `frontend/core/pow.js` and are covered by
Node's `--test` runner; without a rendered challenge the entry does nothing.

## Internationalization

The UI translations live in `server/locales/` (`en.toml`, `zh.toml`, plus the email
templates `email_en.toml` / `email_zh.toml`) and are compiled in with
`include_str!`. `[ui] default_language` selects the fallback when a visitor has no
preference.

## Tests

```bash
node --test "server/frontend/**/*.test.js"
```

Browser end-to-end tests live in `e2e/` and run through Playwright; see
[Testing](/dev/testing).

## Related

- [Architecture](/dev/architecture) — how the frontend fits the server crates.
- [Testing](/dev/testing) — the full suite and CI matrix.
