import { test, expect } from "@playwright/test";
import { createRepo, readState, seedRepo, uploadFile } from "../helpers/api";

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

// A failed "load more" must not consume the page it was fetching: `search.js` used to increment the
// page number before the request, so one 500 skipped a whole page of results.
test("a failed page load does not skip results", async ({ page }) => {
  const state = readState();
  const repoId = await createRepo(state.baseURL, state.adminToken, "search-paging-repo");
  // 21 files: one full page of 20, and one left over for the second page.
  const files = Array.from({ length: 21 }, (_, i) => `pagedonly-${String(i + 1).padStart(2, "0")}.txt`);
  for (const name of files) {
    await uploadFile(state.baseURL, state.adminToken, repoId, "/", name, `content ${name}\n`);
  }

  await page.goto("/search/?q=pagedonly");
  const rows = page.locator("#search-results li");
  // The page renders one full page server-side; the rest arrives through /api2/search/.
  await expect(rows).toHaveCount(20);

  // Break exactly the first "load more" request.
  await page.route("**/api2/search/**", (route) => route.fulfill({ status: 500, body: "boom" }), {
    times: 1,
  });
  const failed = page.waitForResponse(
    (response) => response.url().includes("/api2/search/") && response.status() === 500,
  );
  await page.locator("#scroll-sentinel").scrollIntoViewIfNeeded();
  await failed;
  await expect(page.locator("#loading-spinner")).toBeHidden();
  await expect(rows).toHaveCount(20);

  // The retry must ask for page 2 again, not page 3. The list scrolls inside <main>, so the
  // sentinel has to leave and re-enter that viewport for the observer to fire again.
  await page.locator("main").evaluate((element) => {
    element.scrollTop = 0;
  });
  const retried = page.waitForResponse(
    (response) => response.url().includes("/api2/search/") && response.url().includes("page=2"),
  );
  await page.locator("#scroll-sentinel").scrollIntoViewIfNeeded();
  await retried;
  await expect(rows).toHaveCount(21);
});
