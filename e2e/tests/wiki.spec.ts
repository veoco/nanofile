import { test, expect, type Page } from "@playwright/test";
import { readState, createWiki } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(() => {
  state = readState();
});

/** A wiki card on the /wikis/ list page, filtered by its name. */
const wikiCard = (page: Page, name: string) =>
  page.locator(".card").filter({ hasText: name });

test("wiki list page shows created wikis", async ({ page }) => {
  const name = `list-wiki-${Date.now()}`;
  await createWiki(state.baseURL, state.adminToken, name);

  await page.goto("/wikis/");
  await expect(wikiCard(page, name)).toBeVisible();
});

test("create a wiki from the UI", async ({ page }) => {
  const name = `ui-wiki-${Date.now()}`;

  await page.goto("/wikis/");
  await page.locator('[data-action="show-wiki-create"]').click();
  await expect(page.locator("#wiki-create-overlay")).toBeVisible();
  await page.locator("#wiki-create-input").fill(name);
  await page.locator('#wiki-create-overlay button[type="submit"]').click();

  // Form submission reloads the list page.
  await expect(wikiCard(page, name)).toBeVisible({ timeout: 15_000 });
});

test("rename and delete a wiki from the UI", async ({ page }) => {
  const original = `rename-me-${Date.now()}`;
  const renamed = `renamed-${Date.now()}`;
  await createWiki(state.baseURL, state.adminToken, original);

  await page.goto("/wikis/");
  await expect(wikiCard(page, original)).toBeVisible();

  // Rename.
  await wikiCard(page, original).locator('[data-action="show-wiki-rename"]').click();
  await expect(page.locator("#wiki-rename-overlay")).toBeVisible();
  await page.locator("#wiki-rename-input").fill(renamed);
  await page.locator('#wiki-rename-overlay button[type="submit"]').click();
  await expect(wikiCard(page, renamed)).toBeVisible({ timeout: 15_000 });

  // Delete (confirm the native dialog).
  page.once("dialog", (dialog) => dialog.accept());
  await wikiCard(page, renamed)
    .locator('form[data-confirm-name] button[type="submit"]')
    .click();
  await expect(wikiCard(page, renamed)).toHaveCount(0, { timeout: 15_000 });
});

test("view a wiki and add a sub-page", async ({ page }) => {
  const name = `view-wiki-${Date.now()}`;
  const childName = `child-${Date.now()}`;
  const wikiId = await createWiki(state.baseURL, state.adminToken, name);

  await page.goto(`/wikis/${wikiId}/`);
  await expect(page.locator("h2", { hasText: "home" })).toBeVisible();

  // Add a sub-page under the home page.
  await page
    .locator('[data-action="show-page-create"][data-insert-position="inner"]')
    .click();
  await expect(page.locator("#page-create-overlay")).toBeVisible();
  await page.locator("#page-create-input").fill(childName);
  await page.locator('#page-create-overlay button[type="submit"]').click();

  // Redirected to the new page; its name appears in the header.
  await expect(page.locator("h2", { hasText: childName })).toBeVisible({ timeout: 15_000 });
});
