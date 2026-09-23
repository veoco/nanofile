import { test, expect } from "@playwright/test";
import { signInAs } from "../helpers/users";

/**
 * System management: the settings pages.
 *
 * The pages are rendered from the server's settings catalog, so these tests pin
 * the properties an operator relies on rather than a fixed set of fields: every
 * area renders, each row says where its value came from, a live change takes
 * effect without a restart, and a value the environment owns cannot be edited
 * away.
 */

const AREAS = ["", "security/", "storage/", "email/", "advanced/"];

test("the admin menu reaches system management", async ({ page }) => {
  await page.goto("/libraries/");
  await page.locator(".js-user-menu-button").click();
  await page.locator('.js-user-menu-dropdown a[href="/sysadmin/settings/"]').click();
  await expect(page).toHaveURL(/\/sysadmin\/settings\/$/);
});

test("every area renders its rows and its section bar", async ({ page }) => {
  for (const area of AREAS) {
    await page.goto(`/sysadmin/settings/${area}`);
    await expect(page.locator("main .page-title")).toBeVisible();
    // The section bar is how an operator moves between areas.
    await expect(page.locator("main .nf-tabs a").first()).toBeVisible();
    // Every row carries an origin badge, and no label fell back to its key.
    const badges = await page.locator("main .badge").count();
    expect(badges, `${area || "general"} rendered no rows`).toBeGreaterThan(1);
    const body = (await page.locator("main").innerText()).trim();
    expect(body).not.toContain("setting.");
  }
});

test("a live setting takes effect immediately and says where it came from", async ({
  page,
}) => {
  await page.goto("/sysadmin/settings/security/");
  const form = page.locator('form[action="/sysadmin/settings/security/save/"]');
  // Anonymous share links: a live setting the page can show the effect of.
  // The row also carries the hidden `false` a checkbox submits when it is off,
  // so the control has to be named by its type.
  await form
    .locator('input[type="checkbox"][name="server.share_link_enabled"]')
    .uncheck();
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();

  const row = page.locator('[data-setting="server.share_link_enabled"]');
  await expect(row.locator("span.badge", { hasText: "Database" })).toBeVisible();
  // ...and a way back to the file, which is what stops a wrong save being a
  // dead end.
  await row.locator('button[formaction="/sysadmin/settings/reset/"]').click();
  await expect(page.getByText("The saved value was cleared")).toBeVisible();
  await expect(row.locator("span.badge", { hasText: "Default" })).toBeVisible();
});

test("a restart-only setting is marked as such", async ({ page }) => {
  await page.goto("/sysadmin/settings/general/");

  // The bind port is restart-only, and this suite sets it in the environment —
  // which is exactly what makes it read-only here. Both badges have to show.
  const locked = page.locator('[data-setting="server.port"]');
  await expect(locked.locator("span.badge", { hasText: "Needs a restart" })).toBeVisible();
  await expect(locked.locator("span.badge", { hasText: "Environment" })).toBeVisible();
  await expect(page.locator('input[name="server.port"]')).toHaveCount(0);

  // A restart-only setting this run leaves to the config file *is* editable, and
  // saving it says it waits for a restart rather than pretending it applied.
  const row = page.locator('[data-setting="server.max_json_body_mb"]');
  await expect(row.locator("span.badge", { hasText: "Needs a restart" })).toBeVisible();
  const field = row.locator('input[name="server.max_json_body_mb"]');
  const before = await field.inputValue();
  expect(before).not.toBe("7");

  await field.fill("7");
  await page
    .locator('form[action="/sysadmin/settings/general/save/"]')
    .getByRole("button", { name: "Save", exact: true })
    .click();
  // Three things say it: the save confirmation, the page-wide reminder, and the
  // button that applies it.
  await expect(page.getByText(/need a restart/).first()).toBeVisible();
  await expect(page.getByText(/Restart server button/).first()).toBeVisible();
  await expect(page.locator("[data-restart-button]")).toBeVisible();

  // Put it back: the rest of the suite must see the value it booted with.
  await row.locator('button[formaction="/sysadmin/settings/reset/"]').click();
  await expect(page.getByText("The saved value was cleared")).toBeVisible();
  await expect(
    page.locator('input[name="server.max_json_body_mb"]'),
  ).toHaveValue(before);
});

test("an environment-owned row names its variable once", async ({ page }) => {
  // This suite exports NANOFILE_SERVER_PORT, so the bind port is owned by the
  // environment: the row has to say which variable, exactly once, next to the
  // badge that already says where the value came from.
  await page.goto("/sysadmin/settings/");
  const row = page.locator('[data-setting="server.port"]');
  await expect(row.locator("span.badge", { hasText: "Environment" })).toBeVisible();
  const text = await row.innerText();
  expect(text.match(/NANOFILE_SERVER_PORT/g)).toHaveLength(1);
});

test("a read-only value is shown but has no control", async ({ page }) => {
  await page.goto("/sysadmin/settings/advanced/");
  const row = page.locator('[data-setting="database.url"]');
  await expect(row.locator("span.badge", { hasText: "Read-only" })).toBeVisible();
  // The value is visible...
  await expect(row.locator("[data-setting-value]")).toContainText("sqlite:");
  // ...and the secret about the master field is that it has no input at all.
  await page.goto("/sysadmin/settings/security/");
  await expect(page.locator('input[name="server.secret_key"]')).toHaveCount(0);
  await expect(page.locator('input[name="secret:server.secret_key"]')).toHaveCount(0);
});

test("a stored secret is never rendered back", async ({ page }) => {
  await page.goto("/sysadmin/settings/email/");
  const field = page.locator('input[name="secret:email.password"]');
  await expect(field).toBeVisible();
  // The field starts empty: the page says whether something is set, not what.
  await expect(field).toHaveValue("");

  await field.fill("e2e-smtp-secret");
  await page
    .locator('form[action="/sysadmin/settings/email/save/"]')
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(page.getByText("Settings saved.")).toBeVisible();
  // A page reload must not bring it back.
  await page.reload();
  await expect(page.locator('input[name="secret:email.password"]')).toHaveValue("");
  const body = await page.locator("main").innerText();
  expect(body).not.toContain("e2e-smtp-secret");

  // Clear it again: the stub SMTP server needs no credentials.
  await page.locator('input[name="clear:email.password"]').check();
  await page
    .locator('form[action="/sysadmin/settings/email/save/"]')
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(page.getByText("Settings saved.")).toBeVisible();
});

test("a regular account cannot reach the pages", async ({ page, browser }) => {
  const { createUserViaAdmin } = await import("../helpers/users");
  const email = await createUserViaAdmin(page, "settings-visitor", "settings-password-123");
  const visitor = await signInAs(browser, email, "settings-password-123");
  await visitor.page.goto("/sysadmin/settings/");
  await expect(visitor.page).toHaveURL(/\/libraries\//);
  await visitor.close();
});
