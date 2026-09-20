import { test, expect } from "@playwright/test";
import { extractLink, mailFor, waitForMail } from "../helpers/mailbox";
import { createUserViaAdmin, signInAs } from "../helpers/users";

/**
 * The password-reset flow, end to end.
 *
 * Before the mail subsystem existed this spec could only assert on the generic
 * "request received" page, because no link was ever produced — the point of the
 * rewrite is that the link now really arrives, and really changes the password.
 */

test("a reset link arrives by mail and the new password replaces the old one", async ({
  page,
  browser,
}) => {
  const password = "start-password-123";
  const email = await createUserViaAdmin(page, "reset-ok", password);

  await page.goto("/accounts/password/reset/");
  await page.locator("#email").fill(email);
  await page.locator('button[type="submit"]').click();
  // Still the generic page: the response must not say whether the address exists.
  await expect(page.getByText("Request Received")).toBeVisible();

  const mail = await waitForMail({ to: email, subjectIncludes: "Reset your" });
  expect(mail.body).toContain("valid for 3 days");
  // The link is the only copy: it is never in the HTTP response, and only its
  // hash is in the database.
  const link = extractLink(
    mail,
    /http:\/\/127\.0\.0\.1:\d+\/accounts\/password\/reset\/[0-9a-f]{64}\//,
  );

  const visitor = await browser.newContext();
  const confirm = await visitor.newPage();
  await confirm.goto(link);
  await confirm.locator("#password1").fill("changed-password-456");
  await confirm.locator("#password2").fill("changed-password-456");
  await confirm.locator('button[type="submit"]').click();
  await expect(confirm.getByText("All done")).toBeVisible();
  await visitor.close();

  // The old password no longer works…
  const oldLogin = await browser.newContext();
  const oldPage = await oldLogin.newPage();
  await oldPage.goto("/accounts/login/");
  await oldPage.fill('input[name="email"]', email);
  await oldPage.fill('input[name="password"]', password);
  await oldPage.locator('button[type="submit"]').click();
  await expect(oldPage.locator('[role="alert"]')).toBeVisible();
  await oldLogin.close();

  // …and the new one does.
  const { close } = await signInAs(browser, email, "changed-password-456");
  await close();
});

test("an address with no account sends nothing and reports the same page", async ({ page }) => {
  const unknown = `nobody-${Date.now()}@test.local`;

  await page.goto("/accounts/password/reset/");
  await page.locator("#email").fill(unknown);
  await page.locator('button[type="submit"]').click();
  await expect(page.getByText("Request Received")).toBeVisible();

  // Nothing may be queued for it. The wait gives a wrong implementation time to
  // deliver before the assertion passes.
  await page.waitForTimeout(1500);
  expect(mailFor(unknown)).toEqual([]);
});

test("a reset link cannot be used twice", async ({ page, browser }) => {
  const email = await createUserViaAdmin(page, "reset-reuse", "start-password-123");

  await page.goto("/accounts/password/reset/");
  await page.locator("#email").fill(email);
  await page.locator('button[type="submit"]').click();

  const mail = await waitForMail({ to: email, subjectIncludes: "Reset your" });
  const link = extractLink(
    mail,
    /http:\/\/127\.0\.0\.1:\d+\/accounts\/password\/reset\/[0-9a-f]{64}\//,
  );

  const first = await browser.newContext();
  const firstPage = await first.newPage();
  await firstPage.goto(link);
  await firstPage.locator("#password1").fill("changed-password-456");
  await firstPage.locator("#password2").fill("changed-password-456");
  await firstPage.locator('button[type="submit"]').click();
  await expect(firstPage.getByText("All done")).toBeVisible();
  await first.close();

  // The same link again: the token was consumed, so the form is replaced by the
  // invalid-link notice.
  const second = await browser.newContext();
  const secondPage = await second.newPage();
  await secondPage.goto(link);
  await expect(
    secondPage.getByText("This reset link is invalid or has expired."),
  ).toBeVisible();
  await second.close();
});
