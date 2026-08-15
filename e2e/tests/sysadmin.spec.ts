import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/sysadmin/users/");
});

async function createUser(page: import("@playwright/test").Page, suffix: string): Promise<string> {
  const email = `su-${suffix}-${Date.now()}@test.local`;
  await page.locator('button[onclick="openCreate()"]').click();
  await expect(page.locator("#create-overlay")).toBeVisible();
  await page.locator('#create-overlay input[name="email"]').fill(email);
  await page.locator('#create-overlay input[name="password"]').fill("password-123");
  await page.locator('#create-overlay button[type="submit"]').click();
  const row = page.locator("main table tbody tr").filter({ hasText: email });
  await expect(row).toBeVisible();
  return email;
}

const userRow = (page: import("@playwright/test").Page, email: string) =>
  page.locator("main table tbody tr").filter({ hasText: email });

test("create a new user", async ({ page }) => {
  const email = await createUser(page, "create");
  // New users are active by default.
  await expect(userRow(page, email)).toContainText("Active");
});

test("edit a user's active status", async ({ page }) => {
  const email = await createUser(page, "edit");
  const row = userRow(page, email);
  await row.locator('button[onclick^="openEdit"]').click();
  await expect(page.locator("#edit-overlay")).toBeVisible();
  await page.locator("#edit-is-active").uncheck();
  await page.locator('#edit-form button[type="submit"]').click();
  await expect(row).toContainText("Inactive");
});

test("delete a user", async ({ page }) => {
  const email = await createUser(page, "delete");
  const row = userRow(page, email);
  await row.locator('button[onclick^="openDelete"]').click();
  await expect(page.locator("#delete-overlay")).toBeVisible();
  await page.locator('#delete-form button[type="submit"]').click();
  await expect(row).toHaveCount(0);
});
