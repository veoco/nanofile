import { test, expect } from "@playwright/test";
import crypto from "node:crypto";
import { loginViaUI, readState } from "../helpers/api";
import { ADMIN_EMAIL, ADMIN_PASSWORD, BASE_URL } from "../helpers/server";

const B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

function base32Decode(secret: string): Buffer {
  const bits = secret
    .toUpperCase()
    .replace(/=+$/, "")
    .split("")
    .map((c) => B32.indexOf(c));
  const bytes: number[] = [];
  let buffer = 0;
  let bitsLeft = 0;
  for (const b of bits) {
    buffer = (buffer << 5) | b;
    bitsLeft += 5;
    if (bitsLeft >= 8) {
      bytes.push((buffer >> (bitsLeft - 8)) & 0xff);
      bitsLeft -= 8;
    }
  }
  return Buffer.from(bytes);
}

/** RFC 6238 TOTP — SHA1, 6 digits, 30s period (matches the server's totp-rs). */
function totp(secret: string): string {
  const counter = Math.floor(Date.now() / 1000 / 30);
  const buf = Buffer.alloc(8);
  buf.writeUInt32BE(Math.floor(counter / 2 ** 32), 0);
  buf.writeUInt32BE(counter >>> 0, 4);
  const hmac = crypto.createHmac("sha1", base32Decode(secret)).update(buf).digest();
  const offset = hmac[hmac.length - 1] & 0x0f;
  const code =
    ((hmac[offset] & 0x7f) << 24) |
    (hmac[offset + 1] << 16) |
    (hmac[offset + 2] << 8) |
    hmac[offset + 3];
  return String(code % 1000000).padStart(6, "0");
}

/** Return a TOTP code that won't expire mid-request (waits out a short window). */
async function freshTotp(secret: string): Promise<string> {
  const remaining = 30 - (Math.floor(Date.now() / 1000) % 30);
  if (remaining < 4) {
    await new Promise((r) => setTimeout(r, (remaining + 1) * 1000));
  }
  return totp(secret);
}

test("enable 2FA, log in with a backup code, then disable it", async ({ browser }) => {
  // Use a fresh context so the logout/login steps never touch the shared
  // storageState session other specs depend on.
  const ctx = await browser.newContext({ baseURL: BASE_URL });
  const page = await ctx.newPage();
  try {
    await loginViaUI(page, ADMIN_EMAIL, ADMIN_PASSWORD);

    // GET is read-only now (no side effects): start setup via the POST form,
    // then read the pending secret it renders.
    await page.goto("/settings/two-factor/");
    await page
      .locator('form[action="/settings/two-factor/setup/"] button[type="submit"]')
      .click();
    const secret = (await page.locator("code").first().innerText()).trim();
    expect(secret.length).toBeGreaterThan(0);

    // Verify with a freshly-computed TOTP code.
    const code = await freshTotp(secret);
    await page.locator("#code").fill(code);
    await page
      .locator('form[action="/settings/two-factor/verify/"] button[type="submit"]')
      .click();
    await expect(page.locator('form[action="/settings/two-factor/disable/"]')).toBeVisible();

    // Grab a backup code for the login step.
    const backupCode = (await page.locator("code").first().innerText()).trim();
    expect(backupCode.length).toBeGreaterThan(0);

    // Log out, then log in — the flow must ask for a 2FA code.
    await page.goto("/accounts/logout/");
    await page.goto("/accounts/login/");
    await page.fill('input[name="email"]', ADMIN_EMAIL);
    await page.fill('input[name="password"]', ADMIN_PASSWORD);
    await page.locator('button[type="submit"]').click();
    await page.waitForURL(/\/accounts\/two-factor-auth\//);

    // Backup codes are accepted on the 2FA login page.
    await page.locator("#code").fill(backupCode);
    await page
      .locator('form[action="/accounts/two-factor-auth/"] button[type="submit"]')
      .click();
    await page.waitForURL(/\/libraries\//);

    // Disable 2FA (requires the account password).
    await page.goto("/settings/two-factor/");
    await page.locator('form[action="/settings/two-factor/disable/"] #password').fill(ADMIN_PASSWORD);
    await page.locator('form[action="/settings/two-factor/disable/"] button[type="submit"]').click();
    await page.waitForURL(/\/settings\/$/);
    // The overview's security row carries the status; the sidebar shows the
    // same badge, so the assertion has to address the row.
    const securityCard = page.locator('main [data-section="security"]').first();
    await expect(securityCard.getByText("Not configured")).toBeVisible();
  } finally {
    await ctx.close();
  }
});

// The pending half-session is only good for five minutes, and the TOTP form has
// no page to report a dead one on. These check that the reason reaches the
// visitor as a message on the sign-in form instead of a generic "Bad request".
for (const [label, cookieValue] of [
  ["an unknown pending token", "definitely-not-a-real-pending-token"],
  ["a live session token in the pending cookie", "live-session-token"],
] as const) {
  test(`a dead 2FA session is reported on the sign-in form (${label})`, async ({
    browser,
  }) => {
    const state = readState();
    const ctx = await browser.newContext({ baseURL: BASE_URL });
    const page = await ctx.newPage();
    try {
      await ctx.addCookies([
        {
          name: "seahub-session-pending",
          // The second case presents a real, non-pending token, which the page
          // must refuse as proof that the password step happened.
          value: cookieValue === "live-session-token" ? state.adminToken : cookieValue,
          url: BASE_URL,
        },
      ]);

      await page.goto("/accounts/two-factor-auth/");
      await page.locator("#code").fill("123456");
      await page
        .locator('form[action="/accounts/two-factor-auth/"] button[type="submit"]')
        .click();

      await page.waitForURL(/\/accounts\/login\/\?err=/);
      expect(page.url()).toContain("err=auth.session_expired_alt");
      // The sign-in form, with the real reason — not the "Bad request" page.
      await expect(page.locator("#password")).toBeVisible();
      await expect(page.locator('[role="alert"]')).toContainText(
        "Invalid or expired authentication session",
      );
      // The dead cookie is cleared, so a reload does not resubmit it.
      const cookies = await ctx.cookies();
      const pending = cookies.find((c) => c.name === "seahub-session-pending");
      expect(pending?.value ?? "").toBe("");
    } finally {
      await ctx.close();
    }
  });
}
