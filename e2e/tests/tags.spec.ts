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

async function selectFile(page: import("@playwright/test").Page, name: string) {
  await page
    .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] > div:first-child`)
    .click();
}

async function addTag(page: import("@playwright/test").Page, name: string) {
  await selectFile(page, "alpha.txt");
  await expect(page.locator(".js-rp-tags-section")).toBeVisible();
  await page.locator(".js-rp-tag-input").fill(name);
  await page.locator(".js-rp-tag-add").click();
  await expect(page.locator(".js-rp-tag-chip")).toContainText(name);
}

test("add a tag to a file", async ({ page }) => {
  await addTag(page, "mytag");
});

test("remove a tag from the right panel", async ({ page }) => {
  await addTag(page, "mytag");
  await page.locator(".js-rp-tag-chip .js-rp-tag-remove").click();
  await expect(page.locator(".js-rp-tag-chip")).toHaveCount(0);
});

test("filter the folder by tag from the sort bar", async ({ page }) => {
  await addTag(page, "mytag");
  // Tagging refreshes the list; the filter button now appears in the sort bar.
  const filterBtn = page.locator('.js-tag-filter-btn[data-tag="mytag"]');
  await expect(filterBtn).toBeVisible();
  await filterBtn.click();
  await expect(page.locator('.js-sort-bar')).toHaveAttribute("data-tag-filter", "mytag");
  await expect(
    page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"]'),
  ).toBeVisible();
  await expect(
    page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="bravo.txt"]'),
  ).toHaveCount(0);
});

test("click an entry tag chip to filter the folder", async ({ page }) => {
  await addTag(page, "mytag");
  const chip = page.locator(
    '.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-entry-tag[data-tag="mytag"]',
  );
  await expect(chip).toBeVisible();
  await chip.click();
  await expect(page.locator('.js-sort-bar')).toHaveAttribute("data-tag-filter", "mytag");
  await expect(
    page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="bravo.txt"]'),
  ).toHaveCount(0);
});

// Regression: the initial tag-list load must not roll the right panel back
// over a tag the user just saved when that load resolves late (CI flake).
// Uses a distinct file/tag and runs last so the persisted tag doesn't leak
// into the other tests, which share one seeded repo.
test("add a tag completes even when the initial tag list loads slowly", async ({ page }) => {
  await page.route("**/api/v2.1/repos/*/metadata/record/**", async (route) => {
    await new Promise((r) => setTimeout(r, 800));
    await route.continue();
  });
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  // Let the delayed initial-load response land after the save; without the
  // frontend fix it rolls the freshly-added chip back.
  const recDone = page.waitForResponse((r) => r.url().includes("/metadata/record/"));
  await selectFile(page, "bravo.txt");
  await expect(page.locator(".js-rp-tags-section")).toBeVisible();
  await page.locator(".js-rp-tag-input").fill("slowtag");
  await page.locator(".js-rp-tag-add").click();
  await recDone;
  await expect(page.locator(".js-rp-tag-chip")).toContainText("slowtag");
});
