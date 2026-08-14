import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "sort-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("sort by name toggles file order", async ({ page }) => {
  // Directories always sort first, so assert on file rows only.
  const fileRows = page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-type="file"]');
  await expect(fileRows.nth(0)).toHaveAttribute("data-name", "alpha.txt");
  await expect(fileRows.nth(3)).toHaveAttribute("data-name", "delta.txt");
  await page.locator('.js-sort-btn[data-sort="name"]').click();
  await expect(fileRows.nth(0)).toHaveAttribute("data-name", "delta.txt");
  await expect(fileRows.nth(3)).toHaveAttribute("data-name", "alpha.txt");
});
