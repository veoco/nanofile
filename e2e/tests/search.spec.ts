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
