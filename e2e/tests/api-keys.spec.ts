import { test, expect } from "@playwright/test";
import { readState, createRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
  // The scope picker lists the account's libraries; seed one so the page is
  // never empty when this spec runs in isolation.
  await createRepo(state.baseURL, state.adminToken, `key-lib-${Date.now()}`);
});

test.beforeEach(async ({ page }) => {
  await page.goto("/settings/api-keys/");
});

test("the settings index links to the key page", async ({ page }) => {
  await page.goto("/settings/");
  await page.locator('a[href="/settings/api-keys/"]').first().click();
  await expect(page).toHaveURL(/\/settings\/api-keys\/$/);
  await expect(page.locator("h1")).toContainText("API Keys");
});

test("create a key, see it once, then revoke it", async ({ page }) => {
  const name = `e2e-key-${Date.now()}`;
  await page.locator("#create-key-name").fill(name);
  // A read capability and a write capability, plus one library.
  await page.locator('#create-key-form input[name="cap__file.read"]').check();
  await page.locator('#create-key-form input[name="cap__file.write"]').check();
  const repoSelect = page.locator('#create-key-form select[name^="repo__"]').first();
  await repoSelect.selectOption("rw");
  await page.locator("#create-key-expiry").selectOption("30");
  await page.locator('#create-key-form button[type="submit"]').click();

  // The plaintext is shown exactly once.
  await expect(page.locator("#new-key-value")).toBeVisible();
  const secret = await page.locator("#new-key-value").inputValue();
  expect(secret).toHaveLength(40);

  // And the key is listed.
  const card = page.locator(".card").filter({ hasText: name });
  await expect(card).toBeVisible();
  await expect(card).toContainText("file.write");

  // Reload: the secret is gone, the key is not, and no second key appears
  // (the form redirects, so a refresh re-reads the list).
  await page.reload();
  await expect(page.locator("#new-key-value")).toHaveCount(0);
  await expect(page.locator(".card").filter({ hasText: name })).toHaveCount(1);

  // Revoke (native confirm), and it disappears.
  page.once("dialog", (dialog) => dialog.accept());
  await page
    .locator(".card")
    .filter({ hasText: name })
    .locator('form[action$="/revoke/"] button[type="submit"]')
    .click();
  await expect(page.locator(".card").filter({ hasText: name })).toHaveCount(0);
});

test("a preset ticks the matching capabilities", async ({ page }) => {
  await page.locator("#apikey-preset").selectOption("webdav_ro");
  const box = (id: string) => page.locator(`#create-key-form input[name="cap__${id}"]`);
  await expect(box("webdav.read")).toBeChecked();
  await expect(box("file.read")).toBeChecked();
  await expect(box("webdav.write")).not.toBeChecked();
  await expect(box("file.write")).not.toBeChecked();

  // Switching to a write preset also selects the write side.
  await page.locator("#apikey-preset").selectOption("webdav_rw");
  await expect(box("webdav.write")).toBeChecked();
});

test("a key without capabilities is rejected without leaking a secret", async ({ page }) => {
  await page.locator("#create-key-name").fill(`e2e-empty-${Date.now()}`);
  await page.locator("#create-key-expiry").selectOption("30");
  await page.locator('#create-key-form button[type="submit"]').click();
  await expect(page.locator("#new-key-value")).toHaveCount(0);
  await expect(page.getByText("at least one capability")).toBeVisible();
});
