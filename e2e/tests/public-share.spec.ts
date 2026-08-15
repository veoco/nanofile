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
  await expect(page.locator(".file-name")).toHaveText("alpha.txt");
  await expect(page.locator(".download-btn")).toHaveAttribute("href", /\/f\/.*\?dl=1/);
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.locator(".download-btn").click(),
  ]);
  expect(download.suggestedFilename()).toBe("alpha.txt");
});

test("password-protected file share requires the password", async ({ page }) => {
  await page.goto(`/f/${pwFileToken}/`);
  await expect(page.locator('input[name="password"]')).toBeVisible();

  // Wrong password → the form re-renders with an error.
  await page.locator('input[name="password"]').fill("wrong");
  await page.locator('button[type="submit"]').click();
  await expect(page.locator(".error")).toContainText("Incorrect password");

  // Correct password → the file page appears.
  await page.locator('input[name="password"]').fill("secret");
  await page.locator('button[type="submit"]').click();
  await expect(page.locator(".file-name")).toHaveText("bravo.txt");
});

test("directory share lists entries and navigates into subdirectories", async ({ page }) => {
  await page.goto(`/d/${dirToken}/`);
  const alpha = page.locator("a.entry", { hasText: "alpha.txt" });
  const subdir = page.locator("a.entry", { hasText: "subdir/" });
  await expect(alpha).toBeVisible();
  await expect(subdir).toBeVisible();

  // Navigate into the subdirectory via ?p=.
  await subdir.click();
  await expect(page).toHaveURL(/\/d\/.*\/\?p=\/subdir/);
  await expect(page.locator("a.entry", { hasText: "nested.txt" })).toBeVisible();
  // A parent (".. (parent)") link is available.
  await expect(page.locator("a.entry", { hasText: /\(parent\)/ })).toBeVisible();
});

test("directory share downloads the whole folder as a zip", async ({ page }) => {
  await page.goto(`/d/${dirToken}/`);
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.locator(".download-btn").click(),
  ]);
  expect(download.suggestedFilename()).toMatch(/\.zip$/);
});

test("path traversal in a directory share is rejected", async ({ page }) => {
  const resp = await page.request.get(`/d/${dirToken}/?p=/../../..`);
  expect(resp.status()).toBe(400);
});
