import { test, expect } from "@playwright/test";

test("generate and delete an invitation code", async ({ page }) => {
  await page.goto("/settings/invitations/");
  const email = `invite-${Date.now()}@test.local`;
  await page.locator("#email").fill(email);
  await page
    .locator('form[action="/settings/invitations/"] button[type="submit"]')
    .click();
  const card = page.locator("main .nf-prow").filter({ hasText: email });
  await expect(card).toBeVisible();
  await expect(card.locator(".select-all").first()).not.toHaveText("");

  await card.locator('form button[type="submit"]').click();
  await page.locator(".js-confirm-ok").click();
  await expect(card).toHaveCount(0);
});

test("copy a code out of its row", async ({ page }) => {
  // The row hands the code to the clipboard instead of asking the reader to
  // select 32 characters by hand. Record what would be written rather than
  // reading the real clipboard, which needs a browser permission the rest of
  // the suite does not ask for.
  await page.addInitScript(() => {
    (window as any).__copied = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: (text: string) => {
          (window as any).__copied.push(text);
          return Promise.resolve();
        },
      },
    });
  });

  await page.goto("/settings/invitations/");
  const email = `copy-${Date.now()}@test.local`;
  await page.locator("#email").fill(email);
  await page
    .locator('form[action="/settings/invitations/"] button[type="submit"]')
    .click();

  const row = page.locator("main .nf-prow").filter({ hasText: email });
  await expect(row).toBeVisible();
  const code = (await row.locator("code").first().textContent())?.trim();
  expect(code).toBeTruthy();

  await row.locator("[data-copy]").click();
  await expect(page.locator(".nf-toast p")).toHaveText("Copied");
  // The button copies the whole code, not the ellipsised text a narrow row
  // might show.
  expect(await page.evaluate(() => (window as any).__copied)).toEqual([code]);
});
