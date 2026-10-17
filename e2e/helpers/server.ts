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
 * Server secret for this run. Generated rather than committed: it derives the token, TOTP and
 * at-rest block keys, so a fixed value would make every e2e instance share keys.
 */
export const SERVER_SECRET_KEY = crypto.randomBytes(32).toString("hex");

export interface ServerHandle {
  child: ChildProcess;
  baseURL: string;
  tmpRoot: string;
  logPath: string;
}

export interface StartServerOptions {
  /** Port to bind (default: the shared `PORT`). */
  port?: number;
  /** Extra NANOFILE_* env overrides applied on top of the defaults. */
  env?: Record<string, string>;
}

/** Resolve the debug nanofile binary path. The suffix is platform-specific: Windows builds
 * `nanofile.exe`, and a hard-coded `nanofile` fails every spawn with `ENOENT`. */
export function resolveBinary(): string {
  const name = process.platform === "win32" ? "nanofile.exe" : "nanofile";
  return path.resolve(process.cwd(), "..", "target", "debug", name);
}

/**
 * The log directory for one server instance: per instance, so an isolated server's output is
 * not hidden by another's and the teardown audit reads every instance.
 */
function logDirFor(tmpRoot: string): string {
  return path.join(process.cwd(), "test-results", path.basename(tmpRoot));
}

/**
 * Spawn the binary with an isolated temp SQLite DB and storage dirs; stdout/stderr are appended
 * to this instance's log under `test-results/`, where the teardown audit reads them.
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

// Pass the config path via --config so the server does not depend on the working directory.
// The config is generated per run rather than copied: a developer's own config.toml may point
// at a real SMTP server, and its mail settings would be the ones actually used.
  const repoRoot = path.resolve(path.dirname(binaryPath), "..", "..");
  const configPath = path.join(tmpRoot, "config.toml");
// Mail is on for the whole suite, pointed at the stub in helpers/mailbox.ts: the password-reset
// flow needs a real SMTP conversation. These are bootstrap values the settings page supersedes,
// so they are not environment variables, which would win over a saved value.
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
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Server did not become healthy at ${baseURL}`);
}

/**
 * Stop one instance and remove its temp tree. SIGTERM is named on unix, where the server drains
 * on it; Windows has no signals, so the first attempt asks for plain termination there. The
 * fallback is always a plain kill.
 */
export async function stopServer(handle: ServerHandle): Promise<void> {
  handle.child.kill(process.platform === "win32" ? undefined : "SIGTERM");
  await new Promise((r) => setTimeout(r, 500));
  if (handle.child.exitCode === null) {
    handle.child.kill();
  }
  fs.rmSync(handle.tmpRoot, { recursive: true, force: true });
}

export interface SetupHandle {
  child: ChildProcess;
  tmpRoot: string;
  home: string;
  logPath: string;
  setupUrl: string;
}

/**
 * Spawn the binary the way a person starts a fresh install: no `--config`, no `NANOFILE_*`
 * configuration, and a home directory of its own. Every inherited `NANOFILE_*` is removed because
 * presence alone suppresses first-run setup; `NANOFILE_DOTENV=0` is added back because it
 * configures nothing and keeps a developer's `.env` out. The working directory is the temp root.
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

export async function stopSetupServer(handle: SetupHandle): Promise<void> {
  handle.child.kill(process.platform === "win32" ? undefined : "SIGTERM");
  await new Promise((r) => setTimeout(r, 500));
  if (handle.child.exitCode === null) {
    handle.child.kill();
  }
  fs.rmSync(handle.tmpRoot, { recursive: true, force: true });
}
