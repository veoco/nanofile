import { test, expect } from "@playwright/test";

test("generate and delete an invitation code", async ({ page }) => {
  await page.goto("/settings/invitations/");
  const email = `invite-${Date.now()}@test.local`;
  await page.locator("#email").fill(email);
  await page
    .locator('form[action="/settings/invitations/"] button[type="submit"]')
    .click();
  const card = page.locator("main .rounded-lg").filter({ hasText: email });
  await expect(card).toBeVisible();
  await expect(card.locator(".select-all").first()).not.toHaveText("");

  page.once("dialog", (dialog) => dialog.accept());
  await card.locator('form button[type="submit"]').click();
  await expect(card).toHaveCount(0);
});
