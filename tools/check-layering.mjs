#!/usr/bin/env node
// Layering guard: keep the server's module dependency directions honest.
//
// The layers are conventions, not compiler errors, so a `crate::service::...`
// import added inside `repository` compiles and is invisible in review. This
// script scans the source for cross-module references and fails on a direction
// the architecture doc forbids. Two escape hatches exist, both explicit:
// `ALLOWED` (a permanent, reasoned exception) and `DEBT` (a known violation with
// a named file, so a *new* one cannot slip in while the old ones are cleaned up).
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";

const ROOT = process.cwd();
const SERVER_SRC = join(ROOT, "server", "src");

// A module may not reference any of these modules. Directions not listed are
// allowed (e.g. service -> repository, handler -> service, ui -> service).
const RULES = {
  repository: ["service", "handler", "ui", "webdav", "middleware", "tasks", "sandbox", "app", "routes", "fs"],
  domain: ["repository", "service", "handler", "ui", "webdav", "middleware", "tasks", "sandbox", "app", "routes"],
  fs: ["service", "handler", "ui", "webdav", "middleware", "app", "routes", "sandbox"],
  sandbox: ["service", "handler", "ui", "webdav", "middleware", "tasks", "app", "routes"],
  service: ["handler", "ui", "webdav", "middleware", "settings", "app", "routes"],
  tasks: ["handler", "ui", "webdav", "middleware", "app", "routes"],
  handler: ["repository", "sandbox", "app", "routes"],
  middleware: ["handler", "ui", "webdav", "tasks", "sandbox", "app", "routes"],
  ui: ["handler", "app", "routes"],
  webdav: ["handler", "ui", "app", "routes"],
  settings: ["handler", "ui", "webdav", "middleware", "app", "routes"],
};

// Permanent exceptions: each one needs a reason. `files` are path prefixes
// relative to `server/src`.
const ALLOWED = [
  {
    from: "fs",
    to: "tasks",
    files: ["fs/core/gc.rs"],
    reason: "GC runs as a job; JobContext is its progress/cancel port",
  },
  {
    from: "handler",
    to: "fs",
    files: [
      "handler/sync/",
      "handler/trash.rs",
      "handler/web/share_view.rs",
      "handler/web/upload.rs",
      "handler/web/zip_download.rs",
      "handler/web/download.rs",
    ],
    reason: "sync protocol and page/upload adapters consume block/tree functions directly",
  },
  {
    from: "handler",
    to: "tasks",
    files: ["handler/async_batch.rs", "handler/reindex.rs"],
    reason: "entry points that submit and read background jobs",
  },
  {
    from: "service",
    to: "tasks",
    files: ["service/face_tags/", "service/index/", "service/mail/"],
    reason: "job bodies and the index scheduler; the task system is the service layer's executor",
  },
];

// Known debt: current violations that a later change removes. A file listed here
// may keep the edge; any file NOT listed fails. Remove entries as they are fixed.
const DEBT = {
  repository: [],
  domain: [],
  fs: [],
  sandbox: [],
  service: [],
  tasks: [],
  handler: [],
  middleware: [],
  ui: [],
  webdav: [],
};

const TOP_LEVEL = new Set([
  "app", "body_limit", "console", "desktop", "domain", "face_tags", "filters", "fs",
  "handler", "http", "i18n", "indexer", "logging", "middleware", "notification", "ocr",
  "repository", "restart", "routes", "sandbox", "serve", "service", "settings", "setup",
  "startup", "static_assets", "tasks", "thumbnail_util", "tray", "ui", "webdav",
]);

const violations = [];
const debtHits = new Set();

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) out.push(...walk(path));
    else if (entry.endsWith(".rs")) out.push(path);
  }
  return out;
}

// Strip comments and string/char literals; keep byte offsets roughly aligned so
// `#[cfg(test)]` brace matching still works on the cleaned text.
function stripNonCode(text) {
  let out = "";
  let i = 0;
  const n = text.length;
  while (i < n) {
    const c = text[i];
    const next = text[i + 1];
    if (c === "/" && next === "/") {
      while (i < n && text[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && next === "*") {
      i += 2;
      while (i < n && !(text[i] === "*" && text[i + 1] === "/")) i++;
      i += 2;
      continue;
    }
    if (c === '"') {
      i++;
      while (i < n) {
        if (text[i] === "\\") i += 2;
        else if (text[i] === '"') { i++; break; }
        else i++;
      }
      out += " ";
      continue;
    }
    if (c === "r" && (next === '"' || next === "#") && /^r#*"/.test(text.slice(i, i + 4))) {
      const m = /^r(#*)"/.exec(text.slice(i));
      const hashes = m[1];
      const close = '"' + hashes;
      i += m[0].length;
      while (i < n && text.slice(i, i + close.length) !== close) i++;
      i += close.length;
      out += " ";
      continue;
    }
    if (c === "'") {
      // Lifetime (`'a`) or char literal (`'x'`, `'\n'`). Treat `'x'` as a literal.
      if (text[i + 2] === "'" || (text[i + 1] === "\\" && text[i + 3] === "'")) {
        i += text[i + 1] === "\\" ? 4 : 3;
        out += " ";
        continue;
      }
      out += c;
      i++;
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

// Drop `#[cfg(test)] mod ... { ... }` blocks (their imports are not architecture).
function stripTestModules(code) {
  let text = code;
  const re = /#\[cfg\(test\)\]\s*mod\s+\w+\s*\{/g;
  for (;;) {
    const m = re.exec(text);
    if (!m) break;
    let depth = 1;
    let i = re.lastIndex;
    while (i < text.length && depth > 0) {
      if (text[i] === "{") depth++;
      else if (text[i] === "}") depth--;
      i++;
    }
    text = text.slice(0, m.index) + " ".repeat(i - m.index) + text.slice(i);
    re.lastIndex = m.index;
  }
  return text;
}

function moduleOf(file) {
  const rel = relative(SERVER_SRC, file).split(sep).join("/");
  const first = rel.split("/")[0];
  if (first.endsWith(".rs")) return [first.replace(/\.rs$/, ""), rel];
  return [first, rel];
}

function matchesAny(rel, prefixes) {
  return prefixes.some((p) => (p.endsWith("/") ? rel.startsWith(p) : rel === p));
}

for (const file of walk(SERVER_SRC)) {
  const [from, rel] = moduleOf(file);
  if (!RULES[from]) continue;
  let code = readFileSync(file, "utf8");
  code = stripTestModules(stripNonCode(code));
  const lines = code.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const targets = new Set();
    for (const m of line.matchAll(/crate::([a-z_][a-z0-9_]*)/g)) targets.add(m[1]);
    for (const m of line.matchAll(/crate::\{([^}]*)\}/g)) {
      for (const part of m[1].split(",")) {
        const name = part.trim().split(/\s+/)[0];
        if (name) targets.add(name);
      }
    }
    for (const to of targets) {
      if (!TOP_LEVEL.has(to) || to === from) continue;
      const denied = RULES[from]?.includes(to);
      if (!denied) continue;
      const allowed = ALLOWED.some((a) => a.from === from && a.to === to && matchesAny(rel, a.files));
      if (allowed) continue;
      const debtKey = `${from}->${to}`;
      const inDebt = (DEBT[from] ?? []).some((p) => matchesAny(rel, [p]));
      if (inDebt) {
        debtHits.add(`${debtKey} ${rel}`);
        continue;
      }
      violations.push({ file: rel, line: i + 1, from, to, text: line.trim() });
    }
  }
}

if (process.argv.includes("--report")) {
  console.log(`Known debt (${debtHits.size} edges):`);
  for (const hit of [...debtHits].sort()) console.log(`  ${hit}`);
}

if (violations.length) {
  console.error(`Layering violations: ${violations.length}`);
  for (const v of violations) {
    console.error(`  ${v.file}:${v.line}: ${v.from} -> ${v.to}: ${v.text}`);
  }
  console.error(
    "\nIf this is a deliberate, documented exception add it to ALLOWED (with a reason);" +
      " if it is existing debt add the file to DEBT so the debt cannot grow.",
  );
  process.exit(1);
}

console.log(`Layer guard: ok (${debtHits.size} known-debt edges)`);
