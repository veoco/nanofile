import { test, expect } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import {
  resolveBinary,
  startSetupServer,
  stopSetupServer,
  type SetupHandle,
} from "../helpers/server";

/**
* The first-run setup page.
 *
 * An isolated instance started as a person starts a fresh install — no `--config`, no
 * `NANOFILE_*`, an empty home — because only that state has the page at all. The tests stop
 * short of submitting: production PBKDF2 (600k rounds) takes minutes in the test binary, and
 * completion is covered by `server/tests/setup_test.rs`, which lowers the count.
 */
let handle: SetupHandle | undefined;

test.beforeAll(async () => {
  handle = await startSetupServer(resolveBinary());
});

test.afterAll(async () => {
  if (handle) await stopSetupServer(handle);
});

test("a zero-configuration start serves the setup page", async ({ page }) => {
  await page.goto(handle!.setupUrl);

  await expect(page.getByRole("heading", { name: "Set up Nanofile" })).toBeVisible();
  for (const field of [
    'input[name="email"]',
    'input[name="password"]',
    'input[name="password_confirm"]',
    'input[name="addr"]',
    'input[name="port"]',
    'input[name="site_url"]',
    'input[name="data_dir"]',
    'select[name="encryption"]',
  ]) {
    await expect(page.locator(field)).toBeVisible();
  }

  // The page names the file it is about to write, so the operator can find it
  // afterwards without reading the source.
  await expect(page.getByText(path.join(handle!.home, ".nanofile", "config.toml"))).toBeVisible();
});

test("a rejected form comes back with the answers and writes nothing", async ({ page }) => {
  await page.goto(handle!.setupUrl);
  await page.fill('input[name="email"]', "first@example.com");
  await page.fill('input[name="password"]', "secret123");
  await page.fill('input[name="password_confirm"]', "secret123");
  // A URL with a space passes the browser's own validation and is refused by
  // the server, which is what this test is about.
  await page.fill('input[name="site_url"]', "http://two words/");
  await page.locator('form button[type="submit"]').click();

  await expect(page.getByText("Enter an external URL without spaces.")).toBeVisible();
  await expect(page.locator('input[name="email"]')).toHaveValue("first@example.com");
  expect(fs.existsSync(path.join(handle!.home, ".nanofile", "config.toml"))).toBe(false);
});

test("the port answers without handing out the setup token", async ({ page }) => {
  const token = handle!.setupUrl.split("/setup/")[1];
  const origin = new URL(handle!.setupUrl).origin;

  await page.goto(`${origin}/`);
  await expect(page.getByText("Setup in progress")).toBeVisible();
  expect(await page.content()).not.toContain(token);

  const wrong = await page.goto(`${origin}/setup/not-the-token`);
  expect(wrong?.status()).toBe(404);
});

/**
* The whole first run on its own instance: a configuration file under the empty home, an
 * administrator that can sign in, and the listener handed to the real server on the same port.
 * Its own instance because it cannot be reused once setup completes.
 */
test("completing the setup writes the configuration and starts the server", async ({ browser }) => {
  test.setTimeout(120_000);
  const own = await startSetupServer(resolveBinary());
  const url = new URL(own.setupUrl);
  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto(own.setupUrl);
// The server must record what the page prefilled: the default data directory depends on
// whether the binary's directory already has a `data/`, so the test takes the page's answer.
    const dataDir = await page.inputValue('input[name="data_dir"]');
    await page.fill('input[name="email"]', "first@example.com");
    await page.fill('input[name="password"]', "first-password-1");
    await page.fill('input[name="password_confirm"]', "first-password-1");

// The prefilled address is the page's own; if the configured port was taken, the page fell
// back to a random loopback port and the fields must name that instead.
    if (url.port !== "8082") {
      await page.fill('input[name="addr"]', url.hostname);
      await page.fill('input[name="port"]', url.port);
    }
    await page.click('form button[type="submit"]');
    await expect(page.getByText("Setup complete")).toBeVisible({ timeout: 90_000 });

    const configPath = path.join(own.home, ".nanofile", "config.toml");
    expect(fs.existsSync(configPath)).toBe(true);
    const written = fs.readFileSync(configPath, "utf8");
    expect(written).toMatch(/secret_key = "[0-9a-f]{64}"/);
    expect(written).toContain(`port = ${url.port}`);
    expect(written).toContain(path.join(dataDir, "nanofile.db"));

    const login = async () => {
      try {
        const response = await fetch(`${url.origin}/api2/auth-token/`, {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams({
            username: "first@example.com",
            password: "first-password-1",
          }).toString(),
        });
        return response.ok ? response : undefined;
      } catch {
        return undefined;
      }
    };
    let response = await login();
    for (let attempt = 0; !response && attempt < 100; attempt++) {
      await new Promise((r) => setTimeout(r, 100));
      response = await login();
    }
    expect(response, "the account created by the setup can sign in").toBeTruthy();

    await context.close();
  } finally {
    await stopSetupServer(own);
  }
});

