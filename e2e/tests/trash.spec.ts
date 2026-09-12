import { test, expect } from "@playwright/test";
import { deleteRepo, readState, seedRepo, uploadFile } from "../helpers/api";
import type { Page } from "@playwright/test";

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

test("the files tab can empty a library's file trash", async ({ page }) => {
  await deleteAndOpenTrash(page, "empty");
  await page.locator('[data-action="open-clean-trash"]').click();
  await expect(page.locator("#clean-dialog")).toBeVisible();
  await page.locator('[data-action="close-clean"]').click();
  await expect(page.locator("#clean-dialog")).toBeHidden();
});

// ── Deleted libraries ───────────────────────────────────────────────────

/** Seed a library, delete it, and land on the deleted-libraries tab. */
async function deleteLibraryAndOpenTab(
  page: Page,
  suffix: string,
): Promise<{ repoId: string; name: string }> {
  const name = `trash-lib-${suffix}-${Date.now()}`;
  const repoId = await seedRepo(state.baseURL, state.adminToken, name);
  await deleteRepo(state.baseURL, state.adminToken, repoId);
  await page.goto("/trash/?tab=libraries");
  return { repoId, name };
}

const libRow = (page: Page, name: string) =>
  page.locator("#tab-libraries tbody tr").filter({ hasText: name }).first();

test("tab switching shows the deleted libraries", async ({ page }) => {
  const { name } = await deleteLibraryAndOpenTab(page, "tabs");
  await expect(libRow(page, name)).toBeVisible();
  await expect(page.locator("#tab-libraries")).toBeVisible();
  await expect(page.locator("#tab-files")).toBeHidden();

  await page.locator('[data-tab="files"]').click();
  await expect(page).not.toHaveURL(/tab=libraries/);
  await expect(page.locator("#tab-files")).toBeVisible();
  await expect(page.locator("#tab-libraries")).toBeHidden();

  await page.locator('[data-tab="libraries"]').click();
  await expect(page).toHaveURL(/tab=libraries/);
  await expect(libRow(page, name)).toBeVisible();
});

test("restore brings a deleted library back with its files", async ({ page }) => {
  const { repoId, name } = await deleteLibraryAndOpenTab(page, "lib-restore");
  await libRow(page, name).locator('[data-action="restore-lib"]').click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/lib_restored=true/);
  await expect(page.locator("main")).toContainText("Library restored.");
  await expect(libRow(page, name)).toHaveCount(0);

  // The whole library came back, seeded files included.
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await expect(
    page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"]'),
  ).toBeVisible();
});

test("a library can be deleted permanently", async ({ page }) => {
  const { name } = await deleteLibraryAndOpenTab(page, "lib-purge");
  await libRow(page, name).locator('[data-action="delete-lib"]').click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/lib_deleted=true/);
  await expect(page.locator("main")).toContainText("Library permanently deleted.");
  await expect(libRow(page, name)).toHaveCount(0);
});

test("emptying the libraries trash removes every deleted library", async ({ page }) => {
  const first = await deleteLibraryAndOpenTab(page, "lib-empty-a");
  const secondName = `trash-lib-empty-b-${Date.now()}`;
  await deleteRepo(
    state.baseURL,
    state.adminToken,
    await seedRepo(state.baseURL, state.adminToken, secondName),
  );
  await page.goto("/trash/?tab=libraries");
  await expect(libRow(page, first.name)).toBeVisible();
  await expect(libRow(page, secondName)).toBeVisible();

  await page.locator('[data-action="delete-all-libs"]').click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/libs_deleted=true/);
  await expect(page.locator("main")).toContainText("every deleted library is gone");
  await expect(page.locator("#tab-libraries")).toContainText("No deleted libraries.");
});
