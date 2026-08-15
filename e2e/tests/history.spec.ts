import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

const ORIGINAL = "content of alpha.txt (1)\n";

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "history-repo");
  // Overwrite alpha.txt so the file has two revisions (current + original).
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "alpha.txt", "replaced content\n");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("open the file history dialog", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-history-btn')
    .click();
  await expect(page.locator("#history-dialog-overlay")).toBeVisible();
  await expect(page.locator(".js-history-list")).toBeVisible();
});

test("history lists every revision with download and restore actions", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-history-btn')
    .click();
  // Two uploads → two revision entries, each with a Download link + Restore button.
  const revisions = page.locator(".js-history-list .js-history-restore");
  await expect(revisions).toHaveCount(2);
  await expect(page.locator('.js-history-list a[download]')).toHaveCount(2);
});

test("restore an older revision reverts the file content", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-history-btn')
    .click();
  // History is newest-first; the last Restore reverts to the original upload.
  await page.locator(".js-history-list .js-history-restore").last().click();
  await page.locator(".js-confirm-ok").click();
  await expect(page.locator("#history-dialog-overlay")).toBeHidden();
  // The API view of the file should now match the restored revision.
  const res = await page.request.get(`/repos/${repoId}/files/alpha.txt`);
  expect(res.ok()).toBeTruthy();
  expect(await res.text()).toBe(ORIGINAL);
});
