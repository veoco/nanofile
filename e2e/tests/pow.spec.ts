import { test, expect } from "@playwright/test";
import {
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
  resolveBinary,
  startServer,
  stopServer,
} from "../helpers/server";

/**
 * The proof-of-work challenge needs a server with the policy on. A separate
 * instance keeps the shared suite's default (`off`) untouched.
 */
const PORT = 18089;
const URL = `http://127.0.0.1:${PORT}`;

let handle: Awaited<ReturnType<typeof startServer>> | undefined;

test.use({ storageState: { cookies: [], origins: [] } });

test.beforeAll(async () => {
  handle = await startServer(resolveBinary(), {
    port: PORT,
    env: {
      NANOFILE_AUTH_CAPTCHA_MODE: "always",
      // Keep CI fast; production defaults to 17.
      NANOFILE_AUTH_CAPTCHA_DIFFICULTY_BITS: "12",
    },
  });
});

test.afterAll(async () => {
  if (handle) await stopServer(handle);
});

test("the browser solves the challenge and signs in", async ({ page }) => {
  test.setTimeout(60_000);
  await page.goto(`${URL}/accounts/login/`);

  await page.waitForFunction(
    () => {
      const input = document.querySelector('input[name="pow_nonce"]');
      return input instanceof HTMLInputElement && input.value !== "";
    },
    undefined,
    { timeout: 20_000 },
  );

  await page.fill('input[name="email"]', ADMIN_EMAIL);
  await page.fill('input[name="password"]', ADMIN_PASSWORD);
  await page.locator('button[type="submit"]').click();
  await page.waitForURL(`${URL}/libraries/`, { timeout: 15_000 });
});

test("without JavaScript the form is refused", async ({ browser }) => {
  const context = await browser.newContext({
    baseURL: URL,
    javaScriptEnabled: false,
    storageState: { cookies: [], origins: [] },
  });
  const page = await context.newPage();
  try {
    await page.goto("/accounts/login/");
    await page.fill('input[name="email"]', ADMIN_EMAIL);
    await page.fill('input[name="password"]', ADMIN_PASSWORD);
    await page.locator('button[type="submit"]').click();
    await expect(page.locator('[role="alert"]')).toBeVisible();
  } finally {
    await context.close();
  }
});
