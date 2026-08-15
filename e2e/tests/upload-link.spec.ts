import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "ulink-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

async function selectDir(page: import("@playwright/test").Page, name: string) {
  await page
    .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] > div:first-child`)
    .click();
}

async function createUploadLink(page: import("@playwright/test").Page) {
  await selectDir(page, "subdir");
  await page.locator("#rp-upload-link-btn").click();
  await expect(page.locator("#upload-link-dialog-overlay")).toBeVisible();
  await page.locator("#ul-password-input").fill("secret");
  await page.locator("#ul-expiry-select").selectOption("7");
  await page.locator("#ul-description-input").fill("e2e upload link");
  await page.locator("#ul-create-btn").click();
  await expect(page.locator("#ul-link-url")).toHaveValue(/\/u\/[^/]+\//);
}

test("create an upload link for a directory from the right panel", async ({ page }) => {
  await createUploadLink(page);
  await expect(page.locator("#ul-delete-btn")).toBeVisible();
});

test("right panel lists the created upload link", async ({ page }) => {
  await createUploadLink(page);
  // Close the dialog (create button turns into Close after success), then
  // reload and re-select the directory so the panel refetches its links.
  await page.locator("#ul-create-btn").click();
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await selectDir(page, "subdir");
  const links = page.locator(".js-rp-upload-links-list a[href*='/u/']");
  await expect(links.first()).toBeVisible();
});

test("delete an upload link from the dialog", async ({ page }) => {
  await createUploadLink(page);
  page.once("dialog", (dialog) => dialog.accept());
  await page.locator("#ul-delete-btn").click();
  // After deletion the dialog returns to its create state.
  await expect(page.locator("#ul-create-form")).toBeVisible();
  await expect(page.locator("#ul-link-display")).toBeHidden();
});
