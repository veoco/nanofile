import { chromium } from "@playwright/test";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import {
  startServer,
  stopServer,
  BASE_URL,
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
} from "./helpers/server";
import { login } from "./helpers/api";

function ensureBinary(): string {
  const repoRoot = path.resolve(process.cwd(), "..");
  const binary = path.join(repoRoot, "target", "debug", "nanofile");
  if (fs.existsSync(binary)) return binary;
  execFileSync("cargo", ["build"], { cwd: repoRoot, stdio: "inherit" });
  return binary;
}

export default async function globalSetup() {
  const binary = ensureBinary();
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
    const state = { baseURL: BASE_URL, adminEmail: ADMIN_EMAIL, adminPassword: ADMIN_PASSWORD, adminToken };
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
