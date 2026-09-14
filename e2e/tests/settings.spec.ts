import { test, expect } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { login, readState } from "../helpers/api";
import { ADMIN_PASSWORD } from "../helpers/server";

// The settings pages share one shell with a sidebar, so every test starts from
// the overview and walks in the way a person would.
test.beforeEach(async ({ page }) => {
  await page.goto("/settings/");
});

test("the overview links into each settings section", async ({ page }) => {
  await expect(page.locator('a[href="/settings/credentials/"]').first()).toBeVisible();
  await page.locator('nav[aria-label="Settings sections"] a[href="/settings/profile/"]').click();
  await expect(page).toHaveURL(/\/settings\/profile\/$/);
  await expect(page.locator("#display_name")).toBeVisible();
});

test("update the display name", async ({ page }) => {
  await page.goto("/settings/profile/");
  const name = `E2E User ${Date.now()}`;
  await page.locator("#display_name").fill(name);
  await page
    .locator('form[action="/settings/profile/display-name/"] button[type="submit"]')
    .click();
  await expect(page).toHaveURL(/\/settings\/profile\/$/);
  await expect(page.locator("#display_name")).toHaveValue(name);
});

test("upload an avatar", async ({ page }) => {
  await page.goto("/settings/profile/");
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
    "base64",
  );
  await page.setInputFiles("input[name='avatar']", {
    name: "avatar.png",
    mimeType: "image/png",
    buffer: png,
  });
  await page
    .locator('form[action="/settings/profile/avatar/"] button[type="submit"]')
    .click();
  await expect(page).toHaveURL(/\/settings\/profile\/$/);
});

test("changing the password requires the current password and restores it", async ({ page }) => {
  await page.goto("/settings/security/");

  // Wrong current password → the page re-renders with an error alert.
  await page.locator("#old_password").fill("wrong-password");
  await page.locator("#new_password").fill("new-password-123");
  await page
    .locator('form[action="/settings/password/"] button[type="submit"]')
    .click();
  await expect(page.locator('[role="alert"]')).toBeVisible();

  // Correct current password → the security page, with a success notice that
  // says the other credentials were revoked.
  await page.locator("#old_password").fill(ADMIN_PASSWORD);
  await page.locator("#new_password").fill("new-password-123");
  await page
    .locator('form[action="/settings/password/"] button[type="submit"]')
    .click();
  await expect(page).toHaveURL(/\/settings\/security\/\?changed=password$/);
  await expect(page.locator('[role="alert"]')).toContainText("Password changed");

  // Restore the original password so later specs can still log in as admin.
  await page.locator("#old_password").fill("new-password-123");
  await page.locator("#new_password").fill(ADMIN_PASSWORD);
  await page
    .locator('form[action="/settings/password/"] button[type="submit"]')
    .click();
  await expect(page).toHaveURL(/\/settings\/security\/\?changed=password$/);

  // A password change revokes existing API/device tokens (matching seahub's
  // clear_token) while keeping the acting browser session, so refresh the
  // admin API token persisted for later specs.
  const state = readState();
  state.adminToken = await login(state.baseURL, state.adminEmail, ADMIN_PASSWORD);
  fs.writeFileSync(
    path.join(process.cwd(), "test-results", ".e2e-state.json"),
    JSON.stringify(state, null, 2),
  );
});

test("switch the interface language to Chinese and back", async ({ page }) => {
  await page.goto("/settings/profile/");
  await page.locator('select[name="language"]').selectOption("zh");
  await page
    .locator('form[action="/settings/profile/language/"] button[type="submit"]')
    .click();
  await expect(page.getByText("界面语言")).toBeVisible();

  // Restore English so later specs asserting English strings stay stable.
  await page.locator('select[name="language"]').selectOption("en");
  await page
    .locator('form[action="/settings/profile/language/"] button[type="submit"]')
    .click();
  await expect(page.getByText("Interface Language")).toBeVisible();
});
