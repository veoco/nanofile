import { test, expect } from "@playwright/test";
import { extractLink, waitForMail } from "../helpers/mailbox";
import {
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
  resolveBinary,
  startServer,
  stopServer,
} from "../helpers/server";
import { signInAs } from "../helpers/users";

/**
 * Open registration with mandatory email verification.
 *
 * An isolated instance because the test changes the registration mode and needs
 * a mailbox of its own. The shared server keeps the invitation default used by
 * the rest of the suite.
 */
const PORT = 18085;
const LEGACY_PORT = 18086;
const URL = `http://127.0.0.1:${PORT}`;

let handle: Awaited<ReturnType<typeof startServer>> | undefined;
let legacy: Awaited<ReturnType<typeof startServer>> | undefined;

test.use({ storageState: { cookies: [], origins: [] } });

test.beforeAll(async () => {
  handle = await startServer(resolveBinary(), { port: PORT });
});

test.afterAll(async () => {
  if (handle) await stopServer(handle);
});

test("open registration stays closed until the email link is opened", async ({ browser }) => {
  test.setTimeout(60_000);
  const admin = await signInAs(browser, ADMIN_EMAIL, ADMIN_PASSWORD, { baseURL: URL });
  try {
    await admin.page.goto("/sysadmin/settings/authentication/");
    const row = admin.page.locator('[data-setting="auth.require_invitation"]');
    await expect(row).toBeVisible();
    await row.locator('input[type="checkbox"][name="auth.require_invitation"]').uncheck();
    await admin.page
      .locator('form[action="/sysadmin/settings/authentication/save/"]')
      .getByRole("button", { name: "Save", exact: true })
      .click();
    await expect(admin.page.getByText("Settings saved.")).toBeVisible();
  } finally {
    await admin.close();
  }

  const context = await browser.newContext({
    baseURL: URL,
    storageState: { cookies: [], origins: [] },
  });
  const page = await context.newPage();
  const email = `open-${Date.now()}@test.local`;
  const password = "password-123";

  await page.goto("/accounts/register/");
  await expect(page.locator("#invitation_code")).toHaveCount(0);
  await page.locator("#email").fill(email);
  await page.locator("#password1").fill(password);
  await page.locator("#password2").fill(password);
  await page.locator('button[type="submit"]').click();
  await expect(page.getByText("Check your email")).toBeVisible();
  await expect(page).not.toHaveURL(/\/libraries\//);

  // A correct password still does not sign in; the cooldown keeps the first
  // message's link valid.
  await page.goto("/accounts/login/");
  await page.locator('input[name="email"]').fill(email);
  await page.locator('input[name="password"]').fill(password);
  await page.locator('button[type="submit"]').click();
  await expect(page.getByText("Check your email")).toBeVisible();
  await expect(page).not.toHaveURL(/\/libraries\//);

  const mail = await waitForMail({ to: email, subjectIncludes: "Verify your" });
  const link = extractLink(
    mail,
    /http:\/\/127\.0\.0\.1:\d+\/accounts\/verify-email\/[0-9a-f]{64}\//,
  );

  const verifyContext = await browser.newContext({
    baseURL: URL,
    storageState: { cookies: [], origins: [] },
  });
  const verifyPage = await verifyContext.newPage();
  await verifyPage.goto(link);
  await expect(verifyPage.getByText("Email verified")).toBeVisible();
  await verifyContext.close();
  await context.close();

  const { page: signedIn, close } = await signInAs(browser, email, password, { baseURL: URL });
  try {
    await expect(signedIn).toHaveURL(/\/libraries\//);
  } finally {
    await close();
  }
});

test("the retired environment variable still closes registration", async () => {
  legacy = await startServer(resolveBinary(), {
    port: LEGACY_PORT,
    env: { NANOFILE_AUTH_ENABLE_INVITATIONS: "false" },
  });
  try {
    const register = await fetch(`${legacy.baseURL}/accounts/register/`);
    expect(register.status).toBe(404);
    const login = await (await fetch(`${legacy.baseURL}/accounts/login/`)).text();
    expect(login).not.toContain("/accounts/register/");
  } finally {
    if (legacy) await stopServer(legacy);
    legacy = undefined;
  }
});
