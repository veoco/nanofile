import { test, expect } from "@playwright/test";
import crypto from "node:crypto";
import { loginViaUI } from "../helpers/api";
import { ADMIN_EMAIL, ADMIN_PASSWORD } from "../helpers/server";

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
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  try {
    await loginViaUI(page, ADMIN_EMAIL, ADMIN_PASSWORD);

    // GET /settings/two-factor/ auto-starts setup and shows the manual secret.
    await page.goto("/settings/two-factor/");
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
    await expect(page.getByText("Not configured")).toBeVisible();
  } finally {
    await ctx.close();
  }
});
