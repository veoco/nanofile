import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "download-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("download a single file with ?dl=1", async ({ page }) => {
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page
      .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] a[href*="?dl=1"]')
      .first()
      .click(),
  ]);
  expect(download.suggestedFilename()).toBe("alpha.txt");
});

test("download a folder as a zip", async ({ page }) => {
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page
      .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="subdir"] .js-entry-download')
      .click(),
  ]);
  expect(download.suggestedFilename()).toMatch(/\.zip$/);
});

test("batch download selected files as a zip", async ({ page }) => {
  await page.keyboard.down("Control");
  for (const name of ["alpha.txt", "bravo.txt"]) {
    await page
      .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] > div:first-child`)
      .click();
  }
  await page.keyboard.up("Control");

  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.locator("#js-selection-actions .js-batch-download").click(),
  ]);
  expect(download.suggestedFilename()).toMatch(/\.zip$/);
});
