import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

test.beforeAll(async () => {
  const state = readState();
  await seedRepo(state.baseURL, state.adminToken, "search-repo");
});

// seadroid's Pro search path calls GET /api2/search/ with search_repo="all"
// (RepoViewModel.searchPro). The server must treat "all" as a scope keyword,
// not a repo id, or it filters out every repo and returns no results.
test("api2 search with search_repo=all returns matches (seadroid Pro path)", async ({ request }) => {
  const state = readState();
  const res = await request.get("/api2/search/", {
    params: { q: "alpha", search_repo: "all", search_type: "all", page: "1", per_page: "20" },
    headers: { authorization: `Bearer ${state.adminToken}` },
  });
  expect(res.ok()).toBeTruthy();
  const data = await res.json();
  expect(data.total).toBeGreaterThan(0);
  const paths = data.results.map((r: { fullpath: string }) => r.fullpath);
  expect(paths).toContain("/alpha.txt");
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
