import { test, expect } from "@playwright/test";
import { readState } from "../helpers/api";
import { ADMIN_PASSWORD } from "../helpers/server";

test.beforeEach(async ({ page }) => {
  await page.goto("/settings/");
});

test("update the display name", async ({ page }) => {
  const name = `E2E User ${Date.now()}`;
  await page.locator("#display_name").fill(name);
  await page
    .locator('form[action="/settings/display-name/"] button[type="submit"]')
    .click();
  await expect(page.locator("#display_name")).toHaveValue(name);
});

test("upload an avatar", async ({ page }) => {
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
    .locator('form[action="/settings/avatar/"] button[type="submit"]')
    .click();
  await expect(page).toHaveURL(/\/settings\/$/);
});

test("changing the password requires the current password and restores it", async ({ page }) => {
  // Wrong current password → the page re-renders with an error alert.
  await page.locator("#old_password").fill("wrong-password");
  await page.locator("#new_password").fill("new-password-123");
  await page
    .locator('form[action="/settings/password/"] button[type="submit"]')
    .click();
  await expect(page.locator('[role="alert"]')).toBeVisible();

  // Correct current password → redirect back to settings.
  await page.locator("#old_password").fill(ADMIN_PASSWORD);
  await page.locator("#new_password").fill("new-password-123");
  await page
    .locator('form[action="/settings/password/"] button[type="submit"]')
    .click();
  await expect(page).toHaveURL(/\/settings\/$/);

  // Restore the original password so later specs can still log in as admin.
  await page.locator("#old_password").fill("new-password-123");
  await page.locator("#new_password").fill(ADMIN_PASSWORD);
  await page
    .locator('form[action="/settings/password/"] button[type="submit"]')
    .click();
  await expect(page).toHaveURL(/\/settings\/$/);
});

test("switch the interface language to Chinese and back", async ({ page }) => {
  await page.locator('select[name="language"]').selectOption("zh");
  await page
    .locator('form[action="/settings/language/"] button[type="submit"]')
    .click();
  await expect(page.getByText("界面语言")).toBeVisible();

  // Restore English so later specs asserting English strings stay stable.
  await page.locator('select[name="language"]').selectOption("en");
  await page
    .locator('form[action="/settings/language/"] button[type="submit"]')
    .click();
  await expect(page.getByText("Interface Language")).toBeVisible();
});
