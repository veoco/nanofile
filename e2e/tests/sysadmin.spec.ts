import { test, expect } from "@playwright/test";
import { subtitleGap } from "../helpers/layout";

test.beforeEach(async ({ page }) => {
  await page.goto("/sysadmin/users/");
});

// The subtitle has no bottom margin of its own, so the user table must keep its
// own top margin or the two touch.
test("the description sits clear of the user table", async ({ page }) => {
  expect(await subtitleGap(page)).toBeGreaterThanOrEqual(16);
});

async function createUser(page: import("@playwright/test").Page, suffix: string): Promise<string> {
  const email = `su-${suffix}-${Date.now()}@test.local`;
  await page.locator('button[data-action="open-create"]').click();
  await expect(page.locator("#create-overlay")).toBeVisible();
  await page.locator('#create-overlay input[name="email"]').fill(email);
  await page.locator('#create-overlay input[name="password"]').fill("password-123");
  await page.locator('#create-overlay button[type="submit"]').click();
  const row = page.locator("main .nf-prow").filter({ hasText: email });
  await expect(row).toBeVisible();
  return email;
}

const userRow = (page: import("@playwright/test").Page, email: string) =>
  page.locator("main .nf-prow").filter({ hasText: email });

test("create a new user", async ({ page }) => {
  const email = await createUser(page, "create");
  // New users are active by default.
  await expect(userRow(page, email)).toContainText("Active");
});

// A successful action lands back on the list with a confirmation, and a failed
// one re-renders with the reason: neither may pass silently.
test("a successful create is confirmed", async ({ page }) => {
  await createUser(page, "created-banner");
  await expect(page.locator("main .nf-banner.is-ok")).toContainText("User created");
});

test("a failed create reports why", async ({ page }) => {
  const email = await createUser(page, "dupe");
  // The address now exists, so the second create must be rejected.
  await page.locator('button[data-action="open-create"]').click();
  await page.locator('#create-overlay input[name="email"]').fill(email);
  await page.locator('#create-overlay input[name="password"]').fill("password-123");
  await page.locator('#create-overlay button[type="submit"]').click();

  const banner = page.locator("main .nf-banner.is-err");
  await expect(banner).toBeVisible();
  await expect(banner).toContainText("already exists");
});

test("edit a user's active status", async ({ page }) => {
  const email = await createUser(page, "edit");
  const row = userRow(page, email);
  await row.locator('button[data-action="open-edit"]').click();
  await expect(page.locator("#edit-overlay")).toBeVisible();
  await page.locator("#edit-is-active").uncheck();
  await page.locator('#edit-form button[type="submit"]').click();
  await expect(row).toContainText("Inactive");
});

test("delete a user", async ({ page }) => {
  const email = await createUser(page, "delete");
  const row = userRow(page, email);
  await row.locator('button[data-action="open-delete"]').click();
  await expect(page.locator("#delete-overlay")).toBeVisible();
  await page.locator('#delete-form button[type="submit"]').click();
  await expect(row).toHaveCount(0);
});

// The three admin pages dropped their own section bar, so the account menu is
// the only route between them: nothing in the page body points at the other two,
// and a broken link here would strand them. (The menu itself lives in the topbar,
// outside `main`.)
test("the account menu reaches the other admin pages", async ({ page }) => {
  await expect(page.locator('main a[href="/sysadmin/shares/"]')).toHaveCount(0);

  await page.locator(".js-user-menu-button").click();
  await page.locator('.js-user-menu-dropdown a[href="/sysadmin/shares/"]').click();
  await expect(page).toHaveURL(/\/sysadmin\/shares\//);
  await expect(page.locator('main a[href="/sysadmin/users/"]')).toHaveCount(0);

  await page.locator(".js-user-menu-button").click();
  await page.locator('.js-user-menu-dropdown a[href="/sysadmin/tasks/"]').click();
  await expect(page).toHaveURL(/\/sysadmin\/tasks\//);
  await expect(page.locator('main a[href="/sysadmin/users/"]')).toHaveCount(0);
});
