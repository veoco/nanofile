import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

/** Seed a fresh repo, delete a uniquely-named file, then land on /trash/. */
async function deleteAndOpenTrash(
  page: import("@playwright/test").Page,
  suffix: string,
): Promise<{ repoId: string; name: string }> {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `trash-${suffix}-${Date.now()}`);
  const name = `gone-${suffix}.txt`;
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", name, "trash me\n");
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await page
    .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] .js-delete-btn`)
    .click();
  await page.locator(".js-confirm-ok").click();
  await expect(
    page.locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"]`),
  ).toHaveCount(0);
  await page.goto("/trash/");
  return { repoId, name };
}

const trashRow = (page: import("@playwright/test").Page, name: string) =>
  page.locator("main table tbody tr").filter({ hasText: name }).first();

test("deleted files appear in the trash", async ({ page }) => {
  const { name } = await deleteAndOpenTrash(page, "appears");
  await expect(trashRow(page, name)).toBeVisible();
});

test("trash search filters to matching entries", async ({ page }) => {
  const { name } = await deleteAndOpenTrash(page, "search");
  await page.locator('input[name="q"]').fill(name);
  await page.locator('input[name="q"]').press("Enter");
  await page.waitForURL(/\/trash\/\?q=/);
  await expect(trashRow(page, name)).toBeVisible();
  // A seed file that was never deleted must not appear in the trash.
  await expect(trashRow(page, "alpha.txt")).toHaveCount(0);
});

test("restore brings a deleted file back", async ({ page }) => {
  const { repoId, name } = await deleteAndOpenTrash(page, "restore");
  await trashRow(page, name).locator(".js-restore-form button[type='submit']").click();
  await page.locator(".js-confirm-ok").click();
  // The trash page reloads and no longer lists the restored file.
  await expect(trashRow(page, name)).toHaveCount(0);
  // The file is back in the library browser.
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await expect(
    page.locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"]`),
  ).toBeVisible();
});
