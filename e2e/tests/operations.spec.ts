import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";
import { clickRowAction } from "../helpers/details";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "ops-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("delete a file", async ({ page }) => {
  await clickRowAction(page, "alpha.txt", ".js-delete-btn");
  await page.locator(".js-confirm-ok").click();
  await expect(
    page.locator('.js-file-list-view .js-entry-row[data-name="alpha.txt"]'),
  ).toHaveCount(0);
});

test("rename a file", async ({ page }) => {
  await clickRowAction(page, "bravo.txt", ".js-rename-btn");
  await page.locator("#rename-input").fill("bravo2.txt");
  await page.locator('#rename-dialog-form button[type="submit"]').click();
  await expect(
    page.locator('.js-file-list-view .js-entry-row[data-name="bravo2.txt"]'),
  ).toBeVisible({ timeout: 15_000 });
});
