import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "views-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("switches between list, grid, and gallery views", async ({ page }) => {
  await expect(page.locator(".js-file-list-view")).toBeVisible();
  await page.locator(".js-view-grid").click();
  await expect(page.locator(".js-file-grid-view")).toBeVisible();
  await page.locator(".js-view-gallery").click();
  await expect(page.locator(".js-gallery-view")).toBeVisible();
  await page.locator(".js-view-list").click();
  await expect(page.locator(".js-file-list-view")).toBeVisible();
});

test("view mode persists across reload", async ({ page }) => {
  await page.locator(".js-view-grid").click();
  await expect(page.locator(".js-file-grid-view")).toBeVisible();
  await page.reload();
  await expect(page.locator(".js-file-grid-view")).toBeVisible();
});
