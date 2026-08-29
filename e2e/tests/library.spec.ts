import { test, expect } from "@playwright/test";
import { readState, createRepo, createEncryptedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
  // Seed a library so the list page is non-empty. Without this the spec only
  // passes when run as part of the full suite (other specs seed repos first);
  // running it in isolation would time out waiting for a list row.
  await createRepo(state.baseURL, state.adminToken, `seed-lib-${Date.now()}`);
});

async function openLibraries(page: import("@playwright/test").Page): Promise<void> {
  await page.goto("/libraries/");
  await page.waitForSelector('ul[role="list"] li');
}

async function createRepoByName(page: import("@playwright/test").Page, name: string): Promise<string> {
  const repoId = await createRepo(state.baseURL, state.adminToken, name);
  await page.reload();
  await page.waitForSelector('ul[role="list"] li');
  return repoId;
}

test("create a new library", async ({ page }) => {
  await openLibraries(page);
  await page.locator('button[data-action="show-create"]').click();
  await expect(page.locator("#create-overlay")).toBeVisible();
  await page.locator("#create-input").fill(`create-lib-${Date.now()}`);
  await page.locator('#create-overlay button[type="submit"]').click();
  await expect(page.locator("li").filter({ hasText: /create-lib-/ })).toBeVisible({ timeout: 15_000 });
});

test("edit a library name, description and history settings", async ({ page }) => {
  const name = `edit-lib-${Date.now()}`;
  await openLibraries(page);
  await createRepoByName(page, name);
  const li = page.locator("li").filter({ hasText: name });
  await li.locator('button[data-action="show-edit"]').click();
  await expect(page.locator("#edit-overlay")).toBeVisible();
  await page.locator("#edit-name").fill("edited-lib");
  await page.locator("#edit-description").fill("edited description");
  await page.locator("#edit-history-limit").fill("10");
  await page.locator("#edit-history-ttl-days").fill("30");
  await page.locator('#edit-form button[type="submit"]').click();
  await expect(page.locator("li").filter({ hasText: "edited-lib" })).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("li").filter({ hasText: "edited description" })).toBeVisible();
});

test("manage webdav keys from the edit dialog", async ({ page }) => {
  const name = `dav-lib-${Date.now()}`;
  await openLibraries(page);
  await createRepoByName(page, name);
  const li = page.locator("li").filter({ hasText: name });
  await li.locator('button[data-action="show-edit"]').click();
  await expect(page.locator("#edit-overlay")).toBeVisible();
  await page.locator("#webdav-key-name").fill("e2e-device");
  await page.locator("#webdav-key-permission").selectOption("r");
  await page.locator('button[data-action="create-webdav-key"]').click();
  await expect(page.locator("#new-key-box")).toBeVisible();
  await expect(page.locator("#new-key-value")).not.toHaveValue("");
  await expect(page.locator("#webdav-key-list")).toContainText("e2e-device");
  // Read-only badge is shown for the r key.
  await expect(page.locator("#webdav-key-list")).toContainText("Read only");
  // Delete the key (native confirm).
  page.once("dialog", (dialog) => dialog.accept());
  await page.locator("#webdav-key-list button", { hasText: /Delete/ }).click();
  await expect(page.locator("#webdav-key-list")).not.toContainText("e2e-device");
});

test("delete a library", async ({ page }) => {
  const name = `del-lib-${Date.now()}`;
  await openLibraries(page);
  await createRepoByName(page, name);
  const li = page.locator("li").filter({ hasText: name });
  await expect(li).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await li.locator('button[data-action="delete-repo"]').click();
  await expect(li).toHaveCount(0, { timeout: 15_000 });
});

test("renders folder icon for normal and lock icon for encrypted libraries", async ({ page }) => {
  const normalName = `icon-normal-${Date.now()}`;
  const encName = `icon-enc-${Date.now()}`;
  await createRepo(state.baseURL, state.adminToken, normalName);
  await createEncryptedRepo(state.baseURL, state.adminToken, encName, "e2e-password");
  await openLibraries(page);

  // Normal library: brand folder icon block (single svg inside the name link).
  const normalRow = page.locator("li").filter({ hasText: normalName });
  await expect(normalRow.locator("a svg")).toHaveCount(1);

  // Encrypted library: lock icon block + "Encrypted" badge, no folder icon.
  const encRow = page.locator("li").filter({ hasText: encName });
  await expect(encRow.locator("a svg")).toHaveCount(1);
  await expect(encRow).toContainText("Encrypted");
});
