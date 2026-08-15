import { test, expect, type Page } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

const row = (page: Page, name: string) =>
  page.locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"]`);

/** Ctrl-click a set of file rows to multi-select them (list view only). */
async function multiSelect(page: Page, names: string[]) {
  await page.keyboard.down("Control");
  for (const name of names) {
    await page
      .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] > div:first-child`)
      .click();
  }
  await page.keyboard.up("Control");
}

/** Seed an isolated repo so each test is independent of the others. */
async function openFreshRepo(page: Page, suffix: string): Promise<void> {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `batch-${suffix}-${Date.now()}`);
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
}

test("batch delete removes the selected files", async ({ page }) => {
  await openFreshRepo(page, "delete");
  await multiSelect(page, ["alpha.txt", "bravo.txt"]);
  await expect(page.locator("#js-selection-actions")).toBeVisible();
  await page.locator("#js-selection-actions .js-batch-delete").click();
  await page.locator(".js-confirm-ok").click();
  await expect(row(page, "alpha.txt")).toHaveCount(0);
  await expect(row(page, "bravo.txt")).toHaveCount(0);
  await expect(row(page, "charlie.txt")).toBeVisible();
});

test("batch move relocates the selected files into a subdirectory", async ({ page }) => {
  await openFreshRepo(page, "move");
  await multiSelect(page, ["charlie.txt", "delta.txt"]);
  await page.locator("#js-selection-actions .js-batch-move").click();
  await expect(page.locator("#dir-picker-overlay")).toBeVisible();
  // Pick the only directory (subdir) in the picker, then confirm the move.
  await page.locator("#dir-picker-list .js-picker-dir").first().click();
  await page.locator(".js-picker-confirm").click();
  await expect(row(page, "charlie.txt")).toHaveCount(0);
  await expect(row(page, "delta.txt")).toHaveCount(0);
  // Navigate into subdir and verify the files landed there.
  await page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="subdir"] a').first().click();
  await page.waitForSelector('.js-file-list-view:not(.hidden) .js-entry-row[data-name="charlie.txt"]');
  await expect(row(page, "delta.txt")).toBeVisible();
});

test("batch copy duplicates a file into a subdirectory", async ({ page }) => {
  await openFreshRepo(page, "copy");
  await multiSelect(page, ["alpha.txt"]);
  await page.locator("#js-selection-actions .js-batch-copy").click();
  await expect(page.locator("#dir-picker-overlay")).toBeVisible();
  await page.locator("#dir-picker-list .js-picker-dir").first().click();
  await page.locator(".js-picker-confirm").click();
  // Original stays in place…
  await expect(row(page, "alpha.txt")).toBeVisible();
  // …and a copy appears in the subdirectory.
  await page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="subdir"] a').first().click();
  await page.waitForSelector('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"]');
  await expect(row(page, "alpha.txt")).toBeVisible();
});

test("batch reindex reindexes the selected files", async ({ page }) => {
  await openFreshRepo(page, "reindex");
  await multiSelect(page, ["alpha.txt", "bravo.txt"]);
  const btn = page.locator(".js-rp-multi-content .js-batch-reindex");
  await expect(btn).toBeVisible();
  await btn.click();
  // The button is disabled while the reindex runs, then re-enabled.
  await expect(btn).toBeDisabled();
  await expect(btn).toBeEnabled({ timeout: 20_000 });
});
