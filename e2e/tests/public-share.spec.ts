import { test, expect } from "@playwright/test";
import { readState, seedRepo, createShareLink } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;
let fileToken: string;
let pwFileToken: string;
let dirToken: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "public-share-repo");
  fileToken = await createShareLink(state.baseURL, state.adminToken, repoId, "/alpha.txt");
  pwFileToken = await createShareLink(state.baseURL, state.adminToken, repoId, "/bravo.txt", "secret");
  dirToken = await createShareLink(state.baseURL, state.adminToken, repoId, "/");
});

test("file share page shows metadata and downloads the file", async ({ page }) => {
  await page.goto(`/f/${fileToken}/`);
  await expect(page.locator("h1")).toHaveText("alpha.txt");
  // The download action is the page's own `?dl=1` link; assert the href rather
  // than a class, so a restyle cannot silently point it somewhere else.
  const downloadLink = page.getByRole("link", { name: "Download", exact: true });
  await expect(downloadLink).toHaveAttribute("href", /\/f\/.*\?dl=1/);
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    downloadLink.click(),
  ]);
  expect(download.suggestedFilename()).toBe("alpha.txt");
});

test("password-protected file share requires the password", async ({ page }) => {
  await page.goto(`/f/${pwFileToken}/`);
  await expect(page.locator('input[name="password"]')).toBeVisible();

  // Wrong password → the form re-renders with an error.
  await page.locator('input[name="password"]').fill("wrong");
  await page.locator('button[type="submit"]').click();
  await expect(page.locator('[role="alert"]')).toContainText("Incorrect password");

  // Correct password → the file page appears.
  await page.locator('input[name="password"]').fill("secret");
  await page.locator('button[type="submit"]').click();
  await expect(page.locator("h1")).toHaveText("bravo.txt");
});

test("directory share lists entries and navigates into subdirectories", async ({ page }) => {
  await page.goto(`/d/${dirToken}/`);
  const alpha = page.locator("a.nf-prow", { hasText: "alpha.txt" });
  const subdir = page.locator("a.nf-prow", { hasText: "subdir" });
  await expect(alpha).toBeVisible();
  await expect(subdir).toBeVisible();

  // Navigate into the subdirectory via ?p=.
  await subdir.click();
  await expect(page).toHaveURL(/\/d\/.*\/\?p=\/subdir/);
  await expect(page.locator("a.nf-prow", { hasText: "nested.txt" })).toBeVisible();
  // A parent (".. (parent)") link is available.
  await expect(page.locator("a.nf-prow", { hasText: /\(parent\)/ })).toBeVisible();
});

test("directory share downloads the whole folder as a zip", async ({ page }) => {
  await page.goto(`/d/${dirToken}/`);
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    // The folder page's own ZIP link — the entry rows are `?dl=1` too, so the
    // selector has to be the action's name rather than its href.
    page.getByRole("link", { name: "Download ZIP" }).click(),
  ]);
  expect(download.suggestedFilename()).toMatch(/\.zip$/);
});

test("path traversal in a directory share is rejected", async ({ page }) => {
  const resp = await page.request.get(`/d/${dirToken}/?p=/../../..`);
  expect(resp.status()).toBe(400);
});

// The share pages do not load the authenticated UI bundle; their timestamp
// formatting now comes from the shared public-share bundle instead of an inline
// <script> (which is what let script-src drop 'unsafe-inline').
test("directory share formats timestamps without inline scripts", async ({ page }) => {
  await page.goto(`/d/${dirToken}/`);
  await expect(page.locator("[data-ts]").first()).not.toHaveText("");

  const inline = await page.evaluate(() =>
    Array.from(document.querySelectorAll("script"))
      .filter((s) => !s.src && s.type !== "application/json")
      .map((s) => s.textContent || ""),
  );
  expect(inline.join("").trim()).toBe("");
});
