import { test, expect } from "@playwright/test";
import { readState, seedRepo, createShareLink, createUploadLink } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;
const REPO = "sysadmin-shares-repo";

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, REPO);
});

// Other specs create links in their own repos too, so scope every assertion to
// this spec's uniquely-named repo.
const shareRow = (page: import("@playwright/test").Page, name: string) =>
  page
    .locator("#tab-share-links tbody tr")
    .filter({ hasText: REPO })
    .filter({ hasText: name });

const uploadRow = (page: import("@playwright/test").Page) =>
  page
    .locator("#tab-upload-links tbody tr")
    .filter({ hasText: REPO });

test("admin sees and deletes a share link from any user", async ({ page }) => {
  await createShareLink(state.baseURL, state.adminToken, repoId, "/alpha.txt");
  await createShareLink(state.baseURL, state.adminToken, repoId, "/bravo.txt");

  await page.goto("/sysadmin/shares/");
  await expect(shareRow(page, "alpha.txt")).toBeVisible();

  const row = shareRow(page, "bravo.txt");
  page.once("dialog", (dialog) => dialog.accept());
  await row.locator('form.delete-form button[type="submit"]').click();
  await expect(shareRow(page, "bravo.txt")).toHaveCount(0);
});

test("admin sees and deletes an upload link from any user", async ({ page }) => {
  await createUploadLink(state.baseURL, state.adminToken, repoId, "/subdir");

  await page.goto("/sysadmin/shares/?tab=upload-links");
  const row = uploadRow(page);
  await expect(row).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await row.locator('form.delete-form button[type="submit"]').click();
  await expect(uploadRow(page)).toHaveCount(0);
});
