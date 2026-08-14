import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "history-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("open the file history dialog", async ({ page }) => {
  await page
    .locator('.js-file-list-view .js-entry-row[data-name="alpha.txt"] .js-history-btn')
    .click();
  await expect(page.locator("#history-dialog-overlay")).toBeVisible();
  await expect(page.locator(".js-history-list")).toBeVisible();
});
