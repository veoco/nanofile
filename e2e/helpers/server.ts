import { spawn, type ChildProcess } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

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
 * spawn their own isolated instance). */
export function resolveBinary(): string {
  return path.resolve(process.cwd(), "..", "target", "debug", "nanofile");
}

/**
 * Spawn the nanofile binary with an isolated temp SQLite DB + storage dirs.
 * stdout/stderr are appended to test-results/server.log so backend logs are
 * preserved for debugging failures.
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

  const logPath = path.join(process.cwd(), "test-results", "server.log");
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
  // The live `config.toml` is gitignored (it holds the master secret), so a
  // fresh checkout only has `config.toml.example`. Prefer the former when it
  // exists — a developer may have tuned it — and fall back to the template.
  const repoRoot = path.resolve(path.dirname(binaryPath), "..", "..");
  const localConfig = path.join(repoRoot, "config.toml");
  const exampleConfig = path.join(repoRoot, "config.toml.example");
  const configPath = fs.existsSync(localConfig)
    ? localConfig
    : (() => {
        if (!fs.existsSync(exampleConfig)) {
          throw new Error(
            `neither config.toml nor config.toml.example found in ${repoRoot}`,
          );
        }
        const generated = path.join(tmpRoot, "config.toml");
        fs.copyFileSync(exampleConfig, generated);
        return generated;
      })();
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

export async function stopServer(handle: ServerHandle): Promise<void> {
  handle.child.kill("SIGTERM");
  await new Promise((r) => setTimeout(r, 500));
  if (handle.child.exitCode === null) {
    handle.child.kill("SIGKILL");
  }
  fs.rmSync(handle.tmpRoot, { recursive: true, force: true });
}
