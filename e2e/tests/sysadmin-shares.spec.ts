import { test, expect } from "@playwright/test";
import { readState, seedRepo, createShareLink, createUploadLink } from "../helpers/api";
import { bannerGap } from "../helpers/layout";

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
    .locator("#tab-share-links .nf-prow")
    .filter({ hasText: REPO })
    .filter({ hasText: name });

const uploadRow = (page: import("@playwright/test").Page) =>
  page
    .locator("#tab-upload-links .nf-prow")
    .filter({ hasText: REPO });

test("admin sees and deletes a share link from any user", async ({ page }) => {
  await createShareLink(state.baseURL, state.adminToken, repoId, "/alpha.txt");
  await createShareLink(state.baseURL, state.adminToken, repoId, "/bravo.txt");

  await page.goto("/sysadmin/shares/");
  await expect(shareRow(page, "alpha.txt")).toBeVisible();

  const row = shareRow(page, "bravo.txt");
  await row.locator('form.delete-form button[type="submit"]').click();
  await page.locator(".js-confirm-ok").click();
  await expect(shareRow(page, "bravo.txt")).toHaveCount(0);

  // The delete is confirmed, and the banner keeps clear of the description.
  await expect(page.locator("main .nf-banner.is-ok")).toContainText("Link deleted");
  expect(await bannerGap(page)).toBeGreaterThanOrEqual(16);
});

test("admin sees and deletes an upload link from any user", async ({ page }) => {
  await createUploadLink(state.baseURL, state.adminToken, repoId, "/subdir");

  await page.goto("/sysadmin/shares/?tab=upload-links");
  const row = uploadRow(page);
  await expect(row).toBeVisible();
  await row.locator('form.delete-form button[type="submit"]').click();
  await page.locator(".js-confirm-ok").click();
  await expect(uploadRow(page)).toHaveCount(0);

  // The redirect keeps the tab the delete came from, so the empty tab stays put.
  await expect(page).toHaveURL(/tab=upload-links/);
  await expect(page.locator("#tab-upload-links")).toBeVisible();
  await expect(page.locator("main .nf-banner.is-ok")).toContainText("Link deleted");
});

// A browser form must get a page back whatever happens: a token that is already
// gone re-renders the list with the reason, not the API's JSON `error_msg` body.
test("deleting an already-gone link reports instead of answering with JSON", async ({ page }) => {
  await createShareLink(state.baseURL, state.adminToken, repoId, "/csrf.txt");
  await page.goto("/sysadmin/shares/");
  const csrf = await page
    .locator('main form.delete-form input[name="csrf_token"]')
    .first()
    .inputValue();

  const resp = await page.request.post("/sysadmin/shares/share/does-not-exist/delete/", {
    form: { csrf_token: csrf, tab: "share-links" },
  });

  expect(resp.status()).toBe(200);
  expect(resp.headers()["content-type"]).toContain("text/html");
  const body = await resp.text();
  expect(body).toContain("nf-banner is-err");
  expect(body).toContain("no longer exists");
});
