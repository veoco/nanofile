import type { Page } from "@playwright/test";

/**
 * Create an account from the admin page and return its address.
 *
 * The specs need their own accounts rather than the shared admin one: the
 * mailbox is shared by the whole run, so a notification can only be attributed
 * to a recipient that belongs to exactly one test.
 */
export async function createUserViaAdmin(
  page: Page,
  prefix: string,
  password: string,
): Promise<string> {
  const email = `${prefix}-${Date.now()}-${Math.floor(Math.random() * 1000)}@test.local`;
  await page.goto("/sysadmin/users/");
  await page.locator('button[data-action="open-create"]').click();
  await page.locator('#create-overlay input[name="email"]').fill(email);
  await page.locator('#create-overlay input[name="password"]').fill(password);
  await page.locator('#create-overlay button[type="submit"]').click();
  await page.locator("main .nf-prow").filter({ hasText: email }).waitFor();
  return email;
}

/**
 * Sign in as `email` in a fresh context (no shared storage state) and return a
 * page that is already on /libraries/.
 *
 * A fresh context matters for the notification specs: the browser fingerprint is
 * what decides "new sign-in", and reusing a context would carry the previous
 * session's cookies into the check.
 */
export async function signInAs(
  browser: import("@playwright/test").Browser,
  email: string,
  password: string,
  options: import("@playwright/test").BrowserContextOptions = {},
): Promise<{ page: Page; close: () => Promise<void> }> {
  const context = await browser.newContext(options);
  const page = await context.newPage();
  await page.goto("/accounts/login/");
  await page.fill('input[name="email"]', email);
  await page.fill('input[name="password"]', password);
  await page.locator('button[type="submit"]').click();
  await page.waitForURL(/\/libraries\//);
  return { page, close: () => context.close() };
}
