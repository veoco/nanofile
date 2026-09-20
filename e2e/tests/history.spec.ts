import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";
import { clickRowAction } from "../helpers/details";
import { localStamp } from "../helpers/time";

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
  await clickRowAction(page, "alpha.txt", ".js-history-btn");
  await expect(page.locator("#history-dialog-overlay")).toBeVisible();
  await expect(page.locator(".js-history-list")).toBeVisible();
});

test("history lists every revision with download and restore actions", async ({ page }) => {
  await clickRowAction(page, "alpha.txt", ".js-history-btn");
  // Two uploads → two revision entries, each with a Download link + Restore button.
  const revisions = page.locator(".js-history-list .js-history-restore");
  await expect(revisions).toHaveCount(2);
  await expect(page.locator('.js-history-list a[download]')).toHaveCount(2);
});

test("a revision's timestamp uses the page's localized form", async ({ page }) => {
  await clickRowAction(page, "alpha.txt", ".js-history-btn");
  // The dialog is built in JS; its time comes from the same formatter as the
  // file list — a localized stamp in the reader's timezone, not the fixed
  // server-side shape and not `toLocaleString()`.
  const time = page.locator(".js-history-list > div > div > div").nth(1);
  const stamp = time.locator("[data-ts]");
  await expect(stamp).toHaveAttribute("data-ts", /^\d+$/);
  await expect(time).toContainText(await localStamp(stamp));
});

test("restore an older revision reverts the file content", async ({ page }) => {
  await clickRowAction(page, "alpha.txt", ".js-history-btn");
  // History is newest-first; the last Restore reverts to the original upload.
  await page.locator(".js-history-list .js-history-restore").last().click();
  await page.locator(".js-confirm-ok").click();
  await expect(page.locator("#history-dialog-overlay")).toBeHidden();
  // The API view of the file should now match the restored revision.
  const res = await page.request.get(`/repos/${repoId}/files/alpha.txt`);
  expect(res.ok()).toBeTruthy();
  expect(await res.text()).toBe(ORIGINAL);
});
