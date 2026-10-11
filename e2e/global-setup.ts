import { chromium } from "@playwright/test";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import {
  startServer,
  stopServer,
  resolveBinary,
  BASE_URL,
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
} from "./helpers/server";
import { login } from "./helpers/api";
import { startMailServer } from "./helpers/mailbox";

function ensureBinary(): string {
  const repoRoot = path.resolve(process.cwd(), "..");
  // One resolver for the whole suite: the name is platform-specific, and a
  // second copy of the path here is how Windows silently stopped working — the
  // build below would run and the path it returned would still be wrong.
  const binary = resolveBinary();
  if (fs.existsSync(binary)) return binary;
  execFileSync("cargo", ["build"], { cwd: repoRoot, stdio: "inherit" });
  return binary;
}

export default async function globalSetup() {
  const binary = ensureBinary();

  // Clear the previous run's artifacts. Each instance logs into its own
  // directory named after its temp root (see `helpers/server.ts`), because a
  // single shared file is truncated by whichever instance starts last — which
  // is what left the teardown audit reading only the last server's lines.
  const resultsDir = path.join(process.cwd(), "test-results");
  fs.mkdirSync(resultsDir, { recursive: true });
  for (const entry of fs.readdirSync(resultsDir, { withFileTypes: true })) {
    if (entry.isDirectory() && entry.name.startsWith("nanofile-e2e-")) {
      fs.rmSync(path.join(resultsDir, entry.name), { recursive: true, force: true });
    }
  }
  fs.rmSync(path.join(resultsDir, "server.log"), { force: true });

  // The SMTP stub comes first: the server reads its mail settings at startup,
  // and a notification queued before the stub listens would just be a retry.
  await startMailServer();

  const handle = await startServer(binary);

  try {
    // Exchange admin credentials for an API token (used by tests to seed data).
    const adminToken = await login(BASE_URL, ADMIN_EMAIL, ADMIN_PASSWORD);

    // UI login → persist session cookies so browser tests start authenticated.
    const browser = await chromium.launch();
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto(`${BASE_URL}/accounts/login/`);
    await page.fill('input[name="email"]', ADMIN_EMAIL);
    await page.fill('input[name="password"]', ADMIN_PASSWORD);
    await page.locator('button[type="submit"], input[type="submit"]').first().click();
    await page.waitForURL(/\/libraries\//, { timeout: 15_000 });
    const storageState = path.join(process.cwd(), "test-results", "storage-state.json");
    await context.storageState({ path: storageState });
    await browser.close();

    // Persist shared state for tests.
    // Deliberately no adminPassword: the password is a repository constant in
    // helpers/server.ts, and writing it to test-results/ just spreads it into
    // artifacts that people attach to bug reports.
    const state = { baseURL: BASE_URL, adminEmail: ADMIN_EMAIL, adminToken };
    fs.writeFileSync(
      path.join(process.cwd(), "test-results", ".e2e-state.json"),
      JSON.stringify(state, null, 2),
    );

    // Persist server info so teardown can stop it and clean temp dirs.
    fs.writeFileSync(
      path.join(process.cwd(), "test-results", ".server.json"),
      JSON.stringify({ pid: handle.child.pid, tmpRoot: handle.tmpRoot }),
    );
  } catch (err) {
    await stopServer(handle);
    throw err;
  }
}
