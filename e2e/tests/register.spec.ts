import { test, expect, type Browser } from "@playwright/test";

// Registration flows must start unauthenticated.
test.use({ storageState: { cookies: [], origins: [] } });

async function mintInvitationCode(
  browser: Browser,
  boundEmail: string,
): Promise<string> {
  // Mint the code in a throwaway admin context (storageState snapshot) and
  // close it without logging out, so the shared session stays valid.
  const ctx = await browser.newContext({ storageState: "test-results/storage-state.json" });
  try {
    const page = await ctx.newPage();
    await page.goto("/settings/invitations/");
    await page.locator("#email").fill(boundEmail);
    await page
      .locator('form[action="/settings/invitations/"] button[type="submit"]')
      .click();
    const card = page.locator("main .rounded-lg").filter({ hasText: boundEmail });
    await expect(card).toBeVisible();
    const code = (await card.locator(".select-all").first().innerText()).trim();
    expect(code.length).toBeGreaterThan(0);
    return code;
  } finally {
    await ctx.close();
  }
}

test("register with an invitation code auto-logs in", async ({ browser, page }) => {
  // The code is bound to the email it was minted for, so register with that
  // exact email.
  const email = `registered-${Date.now()}@test.local`;
  const code = await mintInvitationCode(browser, email);

  await page.goto("/accounts/register/");
  await page.locator("#email").fill(email);
  await page.locator("#password1").fill("password-123");
  await page.locator("#password2").fill("password-123");
  await page.locator("#invitation_code").fill(code);
  await page.locator('button[type="submit"]').click();
  await page.waitForURL(/\/libraries\//);
});

test("an invalid invitation code is rejected", async ({ page }) => {
  await page.goto("/accounts/register/");
  await page.locator("#email").fill(`bad-${Date.now()}@test.local`);
  await page.locator("#password1").fill("password-123");
  await page.locator("#password2").fill("password-123");
  await page.locator("#invitation_code").fill("invalid-code");
  await page.locator('button[type="submit"]').click();
  // The register endpoint answers with a JSON error_msg.
  await expect(page.getByText("Invalid invitation code")).toBeVisible();
});
