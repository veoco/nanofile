import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

test.beforeAll(async () => {
  const state = readState();
  await seedRepo(state.baseURL, state.adminToken, "search-repo");
});

test("search navigates to the results page", async ({ page }) => {
  await page.goto("/libraries/");
  await page.locator(".js-quick-search").fill("alpha");
  await page.locator(".js-quick-search").press("Enter");
  await page.waitForURL(/\/search\/\?q=alpha/);
});

test("search renders filename matches", async ({ page }) => {
  await page.goto("/search/?q=alpha");
  const results = page.locator("#search-results li");
  await expect(results.first()).toBeVisible();
  await expect(results.locator("a").filter({ hasText: "alpha.txt" }).first()).toBeVisible();
});

test("search with no matches shows the empty state", async ({ page }) => {
  await page.goto("/search/?q=zzz-no-such-file");
  await expect(page.locator("#search-results")).toHaveCount(0);
  await expect(page.getByText(/no results/i)).toBeVisible();
});

test("filename-only toggle updates the query", async ({ page }) => {
  await page.goto("/search/?q=alpha");
  await expect(page.locator("#search-results li").first()).toBeVisible();
  await page.locator('input[name="search_filename_only"]').check();
  await page.waitForURL(/search_filename_only=true/);
  await expect(page.locator("#search-results li").first()).toBeVisible();
});
