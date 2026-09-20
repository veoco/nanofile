import { test, expect } from "@playwright/test";
import { mailFor, waitForMail } from "../helpers/mailbox";
import { createUserViaAdmin, signInAs } from "../helpers/users";

/**
 * The admin email page: settings, the test message, and the outbox.
 *
 * Several specs read mail, and they share one mailbox, so every assertion here
 * is scoped to an address this file invents. Settings are saved back to the
 * values the suite booted with (see `helpers/server.ts`) so a later spec sees
 * the same delivery configuration.
 */

test("the admin menu links to email management", async ({ page }) => {
  await page.goto("/libraries/");
  await page.locator(".js-user-menu-button").click();
  await page.locator('.js-user-menu-dropdown a[href="/sysadmin/email/"]').click();
  await expect(page).toHaveURL(/\/sysadmin\/email\/$/);
  await expect(page.locator("h1")).toContainText("Email Management");
});

test("the page reports delivery as ready and shows the outbox", async ({ page }) => {
  await page.goto("/sysadmin/email/");
  await expect(page.getByText("Can deliver now")).toBeVisible();
  await expect(page.getByText("Ready", { exact: true })).toBeVisible();
  await expect(page.getByText("SMTP settings")).toBeVisible();
  await expect(page.getByText("Message log")).toBeVisible();
  // TLS is off in the e2e configuration, which the page has to say out loud.
  await expect(page.getByText("TLS is off")).toBeVisible();
});

test("a test message is delivered to the given address", async ({ page }) => {
  const to = `probe-${Date.now()}@test.local`;
  await page.goto("/sysadmin/email/");
  await page.locator('form[action="/sysadmin/email/test/"] input[name="to"]').fill(to);
  await page.locator('form[action="/sysadmin/email/test/"] button[type="submit"]').click();
  await expect(page.getByText("Test message sent.")).toBeVisible();

  const mail = await waitForMail({ to, subjectIncludes: "Test message" });
  expect(mail.body).toContain("SMTP settings");
});

test("saving the form persists it and reports the settings as saved here", async ({ page }) => {
  await page.goto("/sysadmin/email/");
  const form = page.locator('form[action="/sysadmin/email/settings/"]');
  await form.locator('input[name="from_name"]').fill("Nanofile E2E Bot");
  await form.locator('button[type="submit"]').click();
  await expect(page.getByText("Email settings saved.")).toBeVisible();
  // Once a row is saved it, not config.toml, is the source of truth.
  await expect(page.getByText("saved here", { exact: true })).toBeVisible();
  await expect(form.locator('input[name="from_name"]')).toHaveValue("Nanofile E2E Bot");

  // A test message now carries the saved sender name, which proves the saved
  // value (not the bootstrap one) is what delivery uses.
  const to = `renamed-${Date.now()}@test.local`;
  await page
    .locator('form[action="/sysadmin/email/test/"] input[name="to"]')
    .fill(to);
  await page.locator('form[action="/sysadmin/email/test/"] button[type="submit"]').click();
  const mail = await waitForMail({ to, subjectIncludes: "Test message" });
  expect(mail.from).toContain("Nanofile E2E Bot");
});

test("a rejected setting is reported on the form", async ({ page }) => {
  await page.goto("/sysadmin/email/");
  const form = page.locator('form[action="/sysadmin/email/settings/"]');
  await form.locator('input[name="from_address"]').fill("not-an-address");
  await form.locator('button[type="submit"]').click();
  await expect(page.locator(".nf-banner.is-err")).toBeVisible();
  await expect(page.locator(".nf-banner.is-err")).toContainText("not a valid email address");

  // Put it back, or every later delivery would be refused.
  await form.locator('input[name="from_address"]').fill("nanofile@test.local");
  await form.locator('button[type="submit"]').click();
  await expect(page.getByText("Email settings saved.")).toBeVisible();
});

test("the outbox lists a delivered message and can delete it", async ({ page }) => {
  const to = `outbox-${Date.now()}@test.local`;
  await page.goto("/sysadmin/email/");
  await page.locator('form[action="/sysadmin/email/test/"] input[name="to"]').fill(to);
  await page.locator('form[action="/sysadmin/email/test/"] button[type="submit"]').click();
  await expect(page.getByText("Test message sent.")).toBeVisible();

  const row = page.locator("main .nf-prow").filter({ hasText: to });
  await expect(row).toBeVisible();
  await expect(row).toContainText("Sent");

  await row.locator('form[action$="/delete/"] button[type="submit"]').click();
  await expect(page.getByText("The log entry was deleted.")).toBeVisible();
  await expect(page.locator("main .nf-prow").filter({ hasText: to })).toHaveCount(0);
});

test("the status filter narrows the log", async ({ page }) => {
  await page.goto("/sysadmin/email/?status=failed");
  await expect(page.getByText("Failed", { exact: true }).first()).toBeVisible();
  // Nothing has failed in this run, so the filtered list is empty while "All"
  // is not.
  await expect(page.locator("main .nf-prow")).toHaveCount(0);
});

test("switching a notification off stops it, and switching it back restores it", async ({
  page,
  browser,
}) => {
  const form = () => page.locator('form[action="/sysadmin/email/settings/"]');
  const email = await createUserViaAdmin(page, "notify-toggle", "toggle-password-123");

  await page.goto("/sysadmin/email/");
  await form().locator('input[name="notify_new_login"]').uncheck();
  await form().locator('button[type="submit"]').click();
  await expect(page.getByText("Email settings saved.")).toBeVisible();

  const muted = await signInAs(browser, email, "toggle-password-123");
  await muted.close();
  await page.waitForTimeout(1500);
  expect(mailFor(email)).toEqual([]);

  // Switch it back on: the next sign-in is announced again.
  await page.goto("/sysadmin/email/");
  await form().locator('input[name="notify_new_login"]').check();
  await form().locator('button[type="submit"]').click();
  await expect(page.getByText("Email settings saved.")).toBeVisible();

  // A different browser: the muted sign-in above already made the default one
  // known, and "new browser" is the switch this test is about.
  const heard = await signInAs(browser, email, "toggle-password-123", {
    userAgent:
      "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
  });
  await heard.close();
  await waitForMail({ to: email, subjectIncludes: "New sign-in" });

  // The reset link is part of the feature, not a switchable notification, so the
  // page offers no switch for it.
  await expect(form().locator('input[name="notify_password_reset"]')).toHaveCount(0);
});
