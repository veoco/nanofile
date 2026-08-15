import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "share-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("create a share link", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-share-btn')
    .click();
  await expect(page.locator("#share-dialog-overlay")).toBeVisible();
  await page.locator(".js-share-confirm").click();
  await expect(page.locator("#share-link-url")).toHaveValue(/\/f\//);
});

test("create a share link with password, expiry and description", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-share-btn')
    .click();
  await page.locator("#share-password-input").fill("secret");
  await page.locator("#share-expiry-select").selectOption("7");
  await page.locator("#share-description-input").fill("e2e share link");
  await page.locator(".js-share-confirm").click();
  await expect(page.locator("#share-link-url")).toHaveValue(/\/f\//);
  await expect(page.locator("#share-delete-btn")).toBeVisible();
});

// Known issue: POST /api/v2.1/share-links/ omits the `s_type` field the
// frontend relies on, so directory share links are built as /f/ URLs instead
// of /d/. Un-skip once the response includes `s_type`.
test.fixme("folder share link uses the /d/ path", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="subdir"] .js-share-btn')
    .click();
  await page.locator(".js-share-confirm").click();
  await expect(page.locator("#share-link-url")).toHaveValue(/\/d\//);
});

test("delete a share link from the dialog", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-share-btn')
    .click();
  await page.locator(".js-share-confirm").click();
  await expect(page.locator("#share-link-url")).toHaveValue(/\/f\//);
  page.once("dialog", (dialog) => dialog.accept());
  await page.locator("#share-delete-btn").click();
  await expect(page.locator("#share-link-display")).toBeHidden();
  await expect(page.locator(".js-share-confirm")).toContainText("Create");
});

test("right panel lists existing share links for a file", async ({ page }) => {
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] .js-share-btn')
    .click();
  await page.locator(".js-share-confirm").click();
  await expect(page.locator("#share-link-url")).toHaveValue(/\/f\//);
  // Close the dialog (confirm button becomes Close after success), then reload
  // and re-select the file so the right panel refetches its share links.
  await page.locator(".js-share-confirm").click();
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="alpha.txt"] > div:first-child')
    .click();
  const links = page.locator(".js-rp-share-links-list a[href*='/f/']");
  await expect(links.first()).toBeVisible();
});
