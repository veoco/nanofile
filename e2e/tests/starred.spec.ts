import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

/** Seed an isolated repo so each test starts with unstarred files. */
async function openFreshRepo(page: import("@playwright/test").Page): Promise<string> {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `starred-${Date.now()}`);
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  return repoId;
}

test("star a file from the row action", async ({ page }) => {
  await openFreshRepo(page);
  const star = page.locator(
    '.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] [data-toggle-star]',
  );
  await expect(star).toHaveAttribute("data-starred", "false");
  await star.click();
  await expect(star).toHaveAttribute("data-starred", "true");
});

test("toggle star from the right panel", async ({ page }) => {
  await openFreshRepo(page);
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="bravo.txt"] > div:first-child')
    .click();
  // openRightPanel replaces the button's className (dropping .js-rp-starred),
  // so the star toggle is located by its data-toggle-star attribute.
  const starBtn = page.locator(".js-rp-content [data-toggle-star]");
  await expect(starBtn).toHaveAttribute("data-starred", "false");
  await starBtn.click();
  await expect(starBtn).toHaveAttribute("data-starred", "true");
  await starBtn.click();
  await expect(starBtn).toHaveAttribute("data-starred", "false");
});

test("starred page lists the starred file", async ({ page }) => {
  await openFreshRepo(page);
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] [data-toggle-star]')
    .click();
  await expect(
    page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] [data-toggle-star]'),
  ).toHaveAttribute("data-starred", "true");
  await page.goto("/starred/");
  await expect(
    page.getByRole("link", { name: /alpha\.txt/ }).first(),
  ).toBeVisible();
});

test("unstar a file from the starred page", async ({ page }) => {
  const repoId = await openFreshRepo(page);
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] [data-toggle-star]')
    .click();
  await page.goto("/starred/");
  // Prior tests leave starred alpha.txt entries behind, so scope to this test's repo.
  const row = page
    .locator("tr")
    .filter({ has: page.locator(`a[href="/libraries/${repoId}/files/alpha.txt"]`) });
  await expect(row).toBeVisible();
  // Accept the native confirm() the unstar button shows.
  page.once("dialog", (dialog) => dialog.accept());
  await row.locator('button[onclick^="unstarItem"]').click();
  await expect(row).toHaveCount(0);
});
