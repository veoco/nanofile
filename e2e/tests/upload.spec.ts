import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "upload-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("upload a file via the hidden file input", async ({ page }) => {
  await page.locator("#file-upload-input").setInputFiles({
    name: "newfile.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("new file content"),
  });
  await expect(
    page.locator('.js-file-list-view .js-entry-row[data-name="newfile.txt"]'),
  ).toBeVisible({ timeout: 20_000 });
});

test("create a new folder", async ({ page }) => {
  await page.locator('button[data-action="new-folder"]').click();
  await page.locator("#new-folder-input").fill("newfolder");
  await page.locator('#new-folder-overlay button[type="submit"]').click();
  await expect(
    page.locator('.js-file-list-view .js-entry-row[data-name="newfolder"]'),
  ).toBeVisible({ timeout: 20_000 });
});
