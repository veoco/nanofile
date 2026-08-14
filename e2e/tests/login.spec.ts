import { test, expect } from "@playwright/test";
import { ADMIN_EMAIL, ADMIN_PASSWORD } from "../helpers/server";

// Login flows must start unauthenticated.
test.use({ storageState: { cookies: [], origins: [] } });

test("unauthenticated access redirects to login", async ({ page }) => {
  await page.goto("/libraries/");
  await expect(page).toHaveURL(/\/accounts\/login\//);
});

test("wrong password shows an error", async ({ page }) => {
  await page.goto("/accounts/login/");
  await page.fill('input[name="email"]', ADMIN_EMAIL);
  await page.fill('input[name="password"]', "wrong-password");
  await page.locator('button[type="submit"]').click();
  await expect(page.locator('[role="alert"]')).toBeVisible();
});

test("valid login redirects to libraries", async ({ page }) => {
  await page.goto("/accounts/login/");
  await page.fill('input[name="email"]', ADMIN_EMAIL);
  await page.fill('input[name="password"]', ADMIN_PASSWORD);
  await page.locator('button[type="submit"]').click();
  await page.waitForURL(/\/libraries\//);
  await expect(page.locator(".js-repo-item").first()).toBeVisible();
});
