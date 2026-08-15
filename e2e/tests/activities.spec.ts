import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

async function openFreshRepo(page: import("@playwright/test").Page): Promise<string> {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  return repoId;
}

const activityRow = (page: import("@playwright/test").Page, name: string) =>
  page.locator("main table tbody tr").filter({ hasText: name }).first();

test("uploading a file records an activity", async ({ page }) => {
  const repoId = await openFreshRepo(page);
  const name = `uploaded-${Date.now()}.txt`;
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", name, "hello\n");
  await page.goto("/activities/");
  await expect(activityRow(page, name)).toBeVisible();
});

test("renaming a file records a rename activity", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  const oldName = `old-${Date.now()}.txt`;
  const newName = `new-${Date.now()}.txt`;
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", oldName, "hello\n");
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await page
    .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${oldName}"] .js-rename-btn`)
    .click();
  await page.locator("#rename-input").fill(newName);
  await page.locator('#rename-dialog-form button[type="submit"]').click();
  await expect(
    page.locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${newName}"]`),
  ).toBeVisible({ timeout: 15_000 });

  await page.goto("/activities/");
  // The rename activity row shows both the old and new names.
  const row = activityRow(page, newName);
  await expect(row).toBeVisible();
  await expect(row).toContainText(oldName);
});

test("deleting a file records a delete activity", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  const name = `deleted-${Date.now()}.txt`;
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", name, "hello\n");
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await page
    .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] .js-delete-btn`)
    .click();
  await page.locator(".js-confirm-ok").click();
  await expect(
    page.locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"]`),
  ).toHaveCount(0);

  await page.goto("/activities/");
  await expect(activityRow(page, name)).toBeVisible();
});
