import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "tags-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("add a tag to a file", async ({ page }) => {
  await page
    .locator('.js-file-list-view .js-entry-row[data-name="alpha.txt"] > div:first-child')
    .click();
  await expect(page.locator(".js-rp-tags-section")).toBeVisible();
  await page.locator(".js-rp-tag-input").fill("mytag");
  await page.locator(".js-rp-tag-add").click();
  await expect(page.locator(".js-rp-tag-chip")).toContainText("mytag");
});
