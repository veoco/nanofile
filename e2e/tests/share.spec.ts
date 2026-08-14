import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "share-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("create a share link", async ({ page }) => {
  await page
    .locator('.js-file-list-view .js-entry-row[data-name="alpha.txt"] .js-share-btn')
    .click();
  await expect(page.locator("#share-dialog-overlay")).toBeVisible();
  await page.locator(".js-share-confirm").click();
  await expect(page.locator("#share-link-url")).toHaveValue(/\/f\//);
});
