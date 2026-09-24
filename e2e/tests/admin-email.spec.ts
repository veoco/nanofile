import { test, expect, type Page } from "@playwright/test";
import { mailFor, waitForMail } from "../helpers/mailbox";
import { createUserViaAdmin, signInAs } from "../helpers/users";

/**
 * The admin email page: the delivery state, the test message and the outbox.
 *
 * The SMTP settings themselves moved to system management, so the tests that
 * change a value drive that page and only *observe* the result here. Several
 * specs read mail, and they share one mailbox, so every assertion is scoped to
 * an address this file invents. Values are saved back to what the suite booted
 * with (see `helpers/server.ts`) so a later spec sees the same configuration.
 */

/** The email section of the system-management page. */
const SETTINGS_PATH = "/sysadmin/settings/email/";
const settingsForm = (page: Page) =>
  page.locator('form[action="/sysadmin/settings/email/save/"]');

test("the admin menu links to email management", async ({ page }) => {
  await page.goto("/libraries/");
  await page.locator(".js-user-menu-button").click();
  await page.locator('.js-user-menu-dropdown a[href="/sysadmin/email/"]').click();
  await expect(page).toHaveURL(/\/sysadmin\/email\/$/);
  await expect(page.locator("h1")).toContainText("Email Management");
});

test("the page reports delivery as ready and points at the settings", async ({
  page,
}) => {
  await page.goto("/sysadmin/email/");
  await expect(page.getByText("Can deliver now")).toBeVisible();
  await expect(page.getByText("Ready", { exact: true })).toBeVisible();
  // The settings section now links to the page that owns them instead of
  // rendering a second, editable copy.
  await expect(page.getByRole("heading", { name: "SMTP settings" })).toBeVisible();
  await expect(page.locator('a[href="/sysadmin/settings/email/"]')).toBeVisible();
  await expect(page.getByText("Message log")).toBeVisible();
  // TLS is off in the e2e configuration, which the page has to say out loud.
  await expect(page.getByText("TLS is off")).toBeVisible();
  // A key with no translation renders as the key itself, so a raw `admin.*`
  // string in the body means a locale entry was deleted while still in use.
  // The dictionary is rendered after `</main>`, so this only sees the page.
  await expect(page.locator("main")).not.toContainText("admin.");
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

test("saving the settings persists it and reports where the value came from", async ({
  page,
}) => {
  await page.goto(SETTINGS_PATH);
  const form = settingsForm(page);
  await form.locator('input[name="email.from_name"]').fill("Nanofile E2E Bot");
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();
  // A saved value is the source of truth for that key, and the page says so.
  await expect(
    page
      .locator('[data-setting="email.from_name"]')
      .locator("span.badge", { hasText: "Database" }),
  ).toBeVisible();
  await expect(form.locator('input[name="email.from_name"]')).toHaveValue(
    "Nanofile E2E Bot",
  );

  // A test message now carries the saved sender name, which proves the saved
  // value (not the bootstrap one) is what delivery uses.
  const to = `renamed-${Date.now()}@test.local`;
  await page.goto("/sysadmin/email/");
  await page
    .locator('form[action="/sysadmin/email/test/"] input[name="to"]')
    .fill(to);
  await page.locator('form[action="/sysadmin/email/test/"] button[type="submit"]').click();
  const mail = await waitForMail({ to, subjectIncludes: "Test message" });
  expect(mail.from).toContain("Nanofile E2E Bot");
});

test("a rejected setting is reported on the form", async ({ page }) => {
  await page.goto(SETTINGS_PATH);
  const form = settingsForm(page);
  await form.locator('input[name="email.from_address"]').fill("not-an-address");
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.locator(".nf-banner.is-err")).toBeVisible();
  await expect(page.locator(".nf-banner.is-err")).toContainText(
    "not a valid email address",
  );

  // Put it back, or every later delivery would be refused.
  await form.locator('input[name="email.from_address"]').fill("nanofile@test.local");
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();
});

test("a saved value can be cleared so the config file applies again", async ({
  page,
}) => {
  await page.goto(SETTINGS_PATH);
  // The test above saved this key, so the row offers to clear it.
  const row = page.locator('[data-setting="email.from_name"]');
  await row.locator('button[formaction="/sysadmin/settings/reset/"]').click();
  await expect(page.getByText("The saved value was cleared")).toBeVisible();
  // Back to the config file, whose value this suite booted with.
  await expect(settingsForm(page).locator('input[name="email.from_name"]')).toHaveValue(
    "Nanofile E2E",
  );
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
  const form = () => settingsForm(page);
  const email = await createUserViaAdmin(page, "notify-toggle", "toggle-password-123");

  await page.goto(SETTINGS_PATH);
  await form()
    .locator('input[type="checkbox"][name="email.notify_new_login"]')
    .uncheck();
  await form().getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();

  const muted = await signInAs(browser, email, "toggle-password-123");
  await muted.close();
  await page.waitForTimeout(1500);
  expect(mailFor(email)).toEqual([]);

  // Switch it back on: the next sign-in is announced again.
  await page.goto(SETTINGS_PATH);
  await form()
    .locator('input[type="checkbox"][name="email.notify_new_login"]')
    .check();
  await form().getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();

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
  await expect(
    form().locator('input[type="checkbox"][name="email.notify_password_reset"]'),
  ).toHaveCount(0);
});
