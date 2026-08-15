import { test, expect } from "@playwright/test";

test("password reset shows the request-received page", async ({ page }) => {
  await page.goto("/accounts/password/reset/");
  await page.locator("#email").fill("reset-me@test.local");
  await page.locator('button[type="submit"]').click();
  // Without an email backend the flow renders the generic done page (no token
  // is minted), so we assert on the page content rather than a redirect.
  await expect(page.getByText("Request Received")).toBeVisible();
});
