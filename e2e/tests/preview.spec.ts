import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

const PNG_1x1 = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "preview-repo");
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "photo.png", PNG_1x1);
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("image preview shows the image", async ({ page }) => {
  await page
    .locator('.js-file-list-view .js-entry-row[data-name="photo.png"] > div:first-child')
    .dblclick();
  await expect(page.locator("#quick-preview-overlay")).toBeVisible();
  await expect(page.locator(".js-qp-img")).toBeVisible();
});
