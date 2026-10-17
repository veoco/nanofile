import { spawn, type ChildProcess } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { SMTP_PORT } from "./mailbox";

export const PORT = 18082;
export const BASE_URL = `http://127.0.0.1:${PORT}`;

export const ADMIN_EMAIL = "e2e-admin@test.local";
export const ADMIN_PASSWORD = "e2e-password-123";

/**
 * Server secret for this run.
 *
 * Generated per run rather than committed as a fixture: the value derives the
 * token, TOTP and at-rest block keys, so a fixed one in the repository would
 * make every e2e instance share keys (and invite copy/paste into production).
 */
export const SERVER_SECRET_KEY = crypto.randomBytes(32).toString("hex");

export interface ServerHandle {
  child: ChildProcess;
  baseURL: string;
  tmpRoot: string;
  logPath: string;
}

/** Options for spawning an additional, isolated server instance. */
export interface StartServerOptions {
  /** Port to bind (default: the shared `PORT`). */
  port?: number;
  /** Extra NANOFILE_* env overrides applied on top of the defaults. */
  env?: Record<string, string>;
}

/** Resolve the debug nanofile binary path (used by global-setup and specs that
 * spawn their own isolated instance).
 *
 * The suffix is platform-specific: Windows builds the file as `nanofile.exe`,
 * and a hard-coded `nanofile` is a path that never exists there — every spec
 * then fails in `spawn` with `ENOENT` rather than in an assertion. */
export function resolveBinary(): string {
  const name = process.platform === "win32" ? "nanofile.exe" : "nanofile";
  return path.resolve(process.cwd(), "..", "target", "debug", name);
}

/**
 * The directory one server instance's log lives in.
 *
 * Per instance rather than one shared file: three specs spawn their own isolated
 * server, and a shared log opened by whichever of them started last would hide
 * the shared instance's lines from the teardown audit. The directory is named
 * after the instance's temp root, which is unique per run.
 */
function logDirFor(tmpRoot: string): string {
  return path.join(process.cwd(), "test-results", path.basename(tmpRoot));
}

/**
 * Spawn the nanofile binary with an isolated temp SQLite DB + storage dirs.
 * stdout/stderr are appended to this instance's log under `test-results/`, so
 * backend logs are preserved for debugging failures — and so the teardown audit
 * reads every instance rather than only the last one to start.
 */
export async function startServer(
  binaryPath: string,
  opts: StartServerOptions = {},
): Promise<ServerHandle> {
  const port = opts.port ?? PORT;
  const baseURL = `http://127.0.0.1:${port}`;
  const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "nanofile-e2e-"));
  const blockDir = path.join(tmpRoot, "blocks");
  const tempDir = path.join(tmpRoot, "temp");
  fs.mkdirSync(blockDir, { recursive: true });
  fs.mkdirSync(tempDir, { recursive: true });

  const logPath = path.join(logDirFor(tmpRoot), "server.log");
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  const logStream = fs.createWriteStream(logPath, { flags: "a" });

  const env = {
    ...process.env,
    NANOFILE_DATABASE_URL: `sqlite:${path.join(tmpRoot, "nanofile.db")}?mode=rwc`,
    NANOFILE_SERVER_ADDR: "127.0.0.1",
    NANOFILE_SERVER_PORT: String(port),
    NANOFILE_SERVER_SITE_URL: baseURL,
    NANOFILE_STORAGE_BLOCK_DIR: blockDir,
    NANOFILE_STORAGE_TEMP_DIR: tempDir,
    NANOFILE_ADMIN_INIT_EMAIL: ADMIN_EMAIL,
    NANOFILE_ADMIN_INIT_PASSWORD: ADMIN_PASSWORD,
    NANOFILE_AUTH_PASSWORD_HASH_ITERATIONS: "1000",
    NANOFILE_SERVER_SECRET_KEY: SERVER_SECRET_KEY,
    NANOFILE_LOG_LEVEL: process.env.E2E_LOG_LEVEL || "info",
    ...opts.env,
  };

  // Pass the config path explicitly via --config so the server does not depend
  // on the working directory. binaryPath is {repoRoot}/target/debug/nanofile.
  //
  // The config file is generated per run rather than copied from the checkout:
  // a developer's own `config.toml` may point at a real SMTP server, and since
  // the environment is only used here for process-level values, mail settings
  // left in a copied file would be the ones actually used.
  const repoRoot = path.resolve(path.dirname(binaryPath), "..", "..");
  const configPath = path.join(tmpRoot, "config.toml");
  // Mail is on for the whole suite, pointed at the stub in `helpers/mailbox.ts`:
  // the password-reset flow and the notifications can only be tested against a
  // real SMTP conversation. These are *bootstrap* values — the settings page
  // supersedes any of them as soon as a spec saves one, which is what the email
  // spec exercises — so they are deliberately not environment variables, which
  // would win over a saved value.
  fs.writeFileSync(
    configPath,
    [
      "[email]",
      "enabled = true",
      'host = "127.0.0.1"',
      `port = ${SMTP_PORT}`,
      'tls = "none"',
      'from_address = "nanofile@test.local"',
      'from_name = "Nanofile E2E"',
      "timeout_secs = 5",
      "max_attempts = 3",
      "",
    ].join("\n"),
  );
  const child = spawn(binaryPath, ["--config", configPath], {
    cwd: repoRoot,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout?.pipe(logStream);
  child.stderr?.pipe(logStream);

  await waitForHealth(baseURL);

  return { child, baseURL, tmpRoot, logPath };
}

async function waitForHealth(baseURL: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${baseURL}/health`);
      if (res.ok) return;
    } catch {
      // server not up yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Server did not become healthy at ${baseURL}`);
}

/**
 * Stop one instance and remove its temp tree.
 *
 * `SIGTERM` is named only where it means something: the server installs a
 * `SIGTERM` handler on unix and drains before it exits, but Windows has no
 * signals at all — `child.kill("SIGTERM")` there is `TerminateProcess`, and
 * naming `SIGKILL` for the fallback would be a second spelling of the same
 * call. So the first attempt names the signal on unix and asks for a plain
 * termination elsewhere, and the fallback is always a plain kill.
 */
export async function stopServer(handle: ServerHandle): Promise<void> {
  handle.child.kill(process.platform === "win32" ? undefined : "SIGTERM");
  await new Promise((r) => setTimeout(r, 500));
  if (handle.child.exitCode === null) {
    handle.child.kill();
  }
  fs.rmSync(handle.tmpRoot, { recursive: true, force: true });
}

/** One instance spawned with no configuration at all, plus the page it served. */
export interface SetupHandle {
  child: ChildProcess;
  tmpRoot: string;
  /** The temporary home directory the first-run setup writes into. */
  home: string;
  logPath: string;
  /** The one-time setup page, scraped from the instance's log. */
  setupUrl: string;
}

/**
 * Spawn the binary the way a person starts a fresh install: no `--config`, no
 * `NANOFILE_*` configuration, and a home directory of its own.
 *
 * Every inherited `NANOFILE_*` variable is removed, because presence alone
 * suppresses the first-run setup — including the ones this suite sets for the
 * shared instance. `NANOFILE_DOTENV=0` is added back deliberately: it is on the
 * allowlist of variables that configure nothing, and it keeps a developer's
 * `.env` from quietly configuring this instance. The working directory is the
 * temp root too, so a `./config.toml` in the checkout cannot be found.
 */
export async function startSetupServer(binaryPath: string): Promise<SetupHandle> {
  const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "nanofile-e2e-setup-"));
  const home = path.join(tmpRoot, "home");
  fs.mkdirSync(home, { recursive: true });

  const logPath = path.join(logDirFor(tmpRoot), "server.log");
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  const logStream = fs.createWriteStream(logPath, { flags: "a" });

  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (key.toUpperCase().startsWith("NANOFILE_")) continue;
    if (value !== undefined) env[key] = value;
  }
  env.HOME = home;
  env.USERPROFILE = home;
  env.NANOFILE_DOTENV = "0";

  const child = spawn(binaryPath, [], {
    cwd: tmpRoot,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout?.pipe(logStream);
  child.stderr?.pipe(logStream);

  const setupUrl = await waitForSetupUrl(logPath);
  return { child, tmpRoot, home, logPath, setupUrl };
}

/** Poll the instance's log for the line that carries the one-time setup URL. */
async function waitForSetupUrl(logPath: string, timeoutMs = 30_000): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (fs.existsSync(logPath)) {
      const text = fs.readFileSync(logPath, "utf8");
      const match = text.match(/http:\/\/127\.0\.0\.1:\d+\/setup\/[0-9a-f]+/);
      if (match) return match[0];
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`a first-run setup URL never appeared in ${logPath}`);
}

/** Stop one zero-configuration instance and remove its temp tree. */
export async function stopSetupServer(handle: SetupHandle): Promise<void> {
  handle.child.kill(process.platform === "win32" ? undefined : "SIGTERM");
  await new Promise((r) => setTimeout(r, 500));
  if (handle.child.exitCode === null) {
    handle.child.kill();
  }
  fs.rmSync(handle.tmpRoot, { recursive: true, force: true });
}
