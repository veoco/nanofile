import { test, expect } from "@playwright/test";
import { signInAs } from "../helpers/users";

/**
 * System management: the settings pages.
 *
 * These pin what an operator relies on: every area renders, each row names where its value came
 * from, a live change takes effect without a restart, and an environment-owned value is read-only.
 */

const AREAS = [
  "",
  "server/",
  "security/",
  "authentication/",
  "rate-limits/",
  "storage/",
  "encryption/",
  "maintenance/",
  "sandbox/",
  "email/",
  "notifications/",
  "advanced/",
];

test("the admin menu reaches system management", async ({ page }) => {
  await page.goto("/libraries/");
  await page.locator(".js-user-menu-button").click();
  await page.locator('.js-user-menu-dropdown a[href="/sysadmin/settings/"]').click();
  await expect(page).toHaveURL(/\/sysadmin\/settings\/$/);
});

/**
 * The panel is the page's own width: it starts at the title's left edge. Inside, a setting is one
 * list row, so its text is inset by the row's padding.
 */
test("the panel lines up with the page header and the filter", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const title = (await page.locator("main .page-title").boundingBox())!;
// The filter's own box: its input starts after the search glyph, so the label is the control
// whose left edge lines up. The box is reached through the filter input it wraps.
  const field = page.locator("label.nf-search:has([data-settings-filter])");
  await expect(field).toHaveCount(1);
  const filter = (await field.boundingBox())!;
  const panel = (await page.locator("form.nf-list").boundingBox())!;
  for (const box of [filter, panel]) {
    expect(Math.abs(box.x - title.x)).toBeLessThan(1);
  }

// The app's boxed search field, not the library list's underlined one: a border on all four
// sides is what separates a page-level search box from the toolbar's filter.
  expect(await field.evaluate((el) => getComputedStyle(el).borderTopWidth)).toBe("1px");

// A row's text starts one border plus one row padding inside the panel; its control column ends
// the same distance from the other edge.
  const row = page.locator('[data-setting="server.addr"]');
  const label = (await row.locator("label").boundingBox())!;
  expect(Math.round(label.x - panel.x)).toBe(15);
  const control = (await row.locator(".nf-srow-act").boundingBox())!;
  expect(Math.round(panel.x + panel.width - (control.x + control.width))).toBe(15);
});

/**
 * The group heading is the panel's own band: full width, with the panel's border drawing its sides,
 * the group's count beside the label, and a hairline under it. The first group leaves its top
 * hairline to the panel's border.
 */
test("a group heading is a full-width band in the panel", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const band = page.locator('[data-setting-group="server_addresses"] .nf-sec');
  const panel = page.locator("form.nf-list");
  const box = (await band.boundingBox())!;
  const outer = (await panel.boundingBox())!;
  expect(Math.abs(box.x - (outer.x + 1))).toBeLessThan(1);
  expect(Math.abs(box.width - (outer.width - 2))).toBeLessThan(1);
  await expect(band).toHaveCSS("border-bottom-width", "1px");
  await expect(band).toHaveCSS("border-top-width", "0px");

  const heading = (await band.locator("h2").boundingBox())!;
  expect(Math.round(heading.x - box.x)).toBe(14);

// 41px: an 11px label at `leading-none`, 14px padding either side and the two hairlines; the
// inherited 1.5 line height would make it 46.5px.
  expect(box.height).toBeLessThanOrEqual(42);
  const middle = (r: { y: number; height: number }) => r.y + r.height / 2;
  expect(Math.abs(middle(heading) - middle(box))).toBeLessThanOrEqual(1);
});

/**
 * A heading's lower hairline closes it, so the first row under it must not draw another — even
 * when filtering has hidden the first child.
 */
test("a group heading is not doubled by its first row's border", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const rows = page.locator('[data-setting-group="server_addresses"] [data-setting]');
  await expect(rows.first()).toHaveCSS("border-top-width", "0px");
  await expect(rows.nth(1)).toHaveCSS("border-top-width", "1px");

  await page.locator("[data-settings-filter]").fill("bind port");
  await expect(rows.first()).toBeHidden();
  const moved = page.locator('[data-setting="server.port"]');
  await expect(moved).toBeVisible();
  await expect(moved).toHaveCSS("border-top-width", "0px");
});

/**
 * Restarting is not a save, so its button is a page-header action: on the title's line and right
 * of it, like the user and email pages.
 */
test("the restart button sits with the page title", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const title = (await page.locator("main .page-title").boundingBox())!;
  const button = (await page.locator("[data-restart-button]").boundingBox())!;
  expect(button.y).toBeLessThan(title.y + title.height);
  expect(button.x).toBeGreaterThan(title.x);
});

test("every area renders its rows and its section bar", async ({ page }) => {
  for (const area of AREAS) {
    await page.goto(`/sysadmin/settings/${area}`);
    await expect(page.locator("main .page-title")).toBeVisible();
    await expect(page.locator("main .nf-tabs a").first()).toBeVisible();
    const badges = await page.locator("main .badge").count();
    expect(badges, `${area || "general"} rendered no rows`).toBeGreaterThan(1);
    const body = (await page.locator("main").innerText()).trim();
    expect(body).not.toContain("setting.");
  }
});

/**
 * The Sandbox page's subject is a measurement: one band per profile and one row per protection,
 * with a warning when that is not the whole set.
 */
test("the sandbox page shows one band per profile and one row per protection", async ({
  page,
}) => {
  await page.goto("/sysadmin/settings/sandbox/");

// One band per measured profile, each with its own state, and no verdict above them: a summary
// would repeat what the badges and item rows already say.
  await expect(page.locator("[data-sandbox-verdict]")).toHaveCount(0);
  await expect(page.locator("[data-sandbox-profile]")).toHaveCount(2);
  await expect(page.locator("[data-sandbox-state]")).toHaveCount(2);
  await expect(page.locator("[data-sandbox-grade]")).toHaveCount(0);
  await expect(page.locator("[data-sandbox-status]")).toHaveCount(0);

// Whether this host can start a confined child is not this test's to decide, so the count comes
// from the bands that were measured; the helper-free parser band is the floor.
  await expect(
    page.locator('[data-sandbox-profile="documents"][data-sandbox-measured="true"]'),
  ).toHaveCount(1);
  const measured = await page.locator('[data-sandbox-measured="true"]').count();
  expect(measured, "no band was measured").toBeGreaterThan(0);

// One row per protection per measured band, in the report's order. A band whose probe never
// answered has no rows, so it cannot contribute a partial set.
  for (const item of ["limits", "files", "network", "process"]) {
    await expect(page.locator(`[data-sandbox-item="${item}"]`)).toHaveCount(measured);
  }
  await expect(page.locator("[data-sandbox-item]")).toHaveCount(measured * 4);

  await expect(page.locator("[data-sandbox-switch]")).toHaveCount(1);
  await expect(
    page.locator('input[type="checkbox"][name="sandbox.enabled"]'),
  ).toHaveCount(1);

  const body = (await page.locator("main").innerText()).trim();
  expect(body).not.toContain("sandbox.");
});

/**
 * The page is a report, not a log: each protection and its consequence is one row, and the
 * platform's residuals are one closed disclosure away.
 */
test("the sandbox page keeps its technical details in disclosures", async ({ page }) => {
  await page.goto("/sysadmin/settings/sandbox/");

// The process row's note is the longest sentence on the page, so it must not be in the flow: the
// media band says the item cannot exist, and an unmeasured band leaves the documents row's note.
  const mediaBand = page.locator(
    '[data-sandbox-profile="media"][data-sandbox-measured="true"]',
  );
  const processRow = (await mediaBand.count())
    ? mediaBand.locator('[data-sandbox-item="process"]')
    : page.locator(
        '[data-sandbox-profile="documents"][data-sandbox-measured="true"] [data-sandbox-item="process"]',
      );
  const note = processRow.locator("details p").first();
  await expect(note).toBeHidden();

  await processRow.locator("details summary").click();
  await expect(note).toBeVisible();

  if (await mediaBand.count()) {
    await expect(processRow.locator("[data-sandbox-media-process]")).toBeVisible();
  }

  const missing = page.locator("[data-sandbox-item]").filter({ hasText: "Missing" });
  for (let i = 0; i < (await missing.count()); i++) {
    await expect(missing.nth(i).locator("[data-sandbox-impact]")).toBeVisible();
  }
});

test("a live setting takes effect immediately and says where it came from", async ({
  page,
}) => {
  await page.goto("/sysadmin/settings/security/");
  const form = page.locator('form[action="/sysadmin/settings/security/save/"]');
// Anonymous share links: a live setting whose effect the page shows. The row also carries the
// hidden `false` a checkbox submits when off, so the control is named by its type.
  await form
    .locator('input[type="checkbox"][name="server.share_download_enabled"]')
    .uncheck();
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Settings saved.")).toBeVisible();

  const row = page.locator('[data-setting="server.share_download_enabled"]');
  await expect(row.locator("span.badge", { hasText: "Database" })).toBeVisible();
  await row.locator('button[formaction="/sysadmin/settings/reset/"]').click();
  await expect(page.getByText("The saved value was cleared")).toBeVisible();
  await expect(row.locator("span.badge", { hasText: "Default" })).toBeVisible();
});

test("a restart-only setting is marked as such", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");

  const locked = page.locator('[data-setting="server.port"]');
  await expect(locked.locator("span.badge", { hasText: "Needs a restart" })).toBeVisible();
  await expect(locked.locator("span.badge", { hasText: "Environment" })).toBeVisible();
  await expect(page.locator('input[name="server.port"]')).toHaveCount(0);

  const row = page.locator('[data-setting="server.max_json_body_mb"]');
  await expect(row.locator("span.badge", { hasText: "Needs a restart" })).toBeVisible();
  const field = row.locator('input[name="server.max_json_body_mb"]');
  const before = await field.inputValue();
  expect(before).not.toBe("7");

  await field.fill("7");
  await page
    .locator('form[action="/sysadmin/settings/server/save/"]')
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(page.getByText(/need a restart/).first()).toBeVisible();
  await expect(page.getByText(/Restart server button/).first()).toBeVisible();
  await expect(page.locator("[data-restart-button]")).toBeVisible();

  await row.locator('button[formaction="/sysadmin/settings/reset/"]').click();
  await expect(page.getByText("The saved value was cleared")).toBeVisible();
  await expect(
    page.locator('input[name="server.max_json_body_mb"]'),
  ).toHaveValue(before);
});

test("an environment-owned row names its variable once", async ({ page }) => {
// NANOFILE_SERVER_PORT is exported by this suite, so the bind port is environment-owned and
// read-only; the row names the variable once, next to the origin badge.
  await page.goto("/sysadmin/settings/");
  const row = page.locator('[data-setting="server.port"]');
  await expect(row.locator("span.badge", { hasText: "Environment" })).toBeVisible();
  const text = await row.innerText();
  expect(text.match(/NANOFILE_SERVER_PORT/g)).toHaveLength(1);
});

test("a row the environment does not own still names its variable", async ({ page }) => {
// The pages are the NANOFILE_* reference: even a defaulted or saved value has to print the
// variable that would override it, because a container has nowhere else to look it up.
  await page.goto("/sysadmin/settings/server/");
  const row = page.locator('[data-setting="server.max_json_body_mb"]');
  await expect(row.locator("span.badge", { hasText: "Environment" })).toHaveCount(0);
  const text = await row.innerText();
  expect(text).toContain("NANOFILE_SERVER_MAX_JSON_BODY_MB");
  expect(text.match(/NANOFILE_SERVER_MAX_JSON_BODY_MB/g)).toHaveLength(1);

  await page.goto("/sysadmin/settings/advanced/");
  const locked = page.locator('[data-setting="database.url"]');
  expect(await locked.innerText()).toContain("NANOFILE_DATABASE_URL");
});

test("a read-only value is shown but has no control", async ({ page }) => {
  await page.goto("/sysadmin/settings/advanced/");
  const row = page.locator('[data-setting="database.url"]');
  await expect(row.locator("span.badge", { hasText: "Read-only" })).toBeVisible();
  await expect(row.locator("[data-setting-value]")).toContainText("sqlite:");
  await page.goto("/sysadmin/settings/security/");
  await expect(page.locator('input[name="server.secret_key"]')).toHaveCount(0);
  await expect(page.locator('input[name="secret:server.secret_key"]')).toHaveCount(0);
});

test("a stored secret is never rendered back", async ({ page }) => {
  await page.goto("/sysadmin/settings/email/");
  const field = page.locator('input[name="secret:email.password"]');
  await expect(field).toBeVisible();
  await expect(field).toHaveValue("");

  await field.fill("e2e-smtp-secret");
  await page
    .locator('form[action="/sysadmin/settings/email/save/"]')
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(page.getByText("Settings saved.")).toBeVisible();
  await page.reload();
  await expect(page.locator('input[name="secret:email.password"]')).toHaveValue("");
  const body = await page.locator("main").innerText();
  expect(body).not.toContain("e2e-smtp-secret");

  await page.locator('input[name="clear:email.password"]').check();
  await page
    .locator('form[action="/sysadmin/settings/email/save/"]')
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(page.getByText("Settings saved.")).toBeVisible();
});

test("a row keeps its internal key out of the text", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const row = page.locator('[data-setting="server.max_upload_size_mb"]');
  const text = await row.innerText();
  expect(text).not.toContain("server.max_upload_size_mb");
  await expect(row.locator("label")).toHaveAttribute(
    "title",
    "server.max_upload_size_mb",
  );
  expect(await row.locator("span.badge").count()).toBeLessThanOrEqual(2);
});

test("a numeric row shows its unit beside the field", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const row = page.locator('[data-setting="server.max_upload_size_mb"]');
  await expect(row.locator('input[type="number"]')).toBeVisible();
  await expect(row.locator("label")).toHaveText("Max upload size");
  await expect(row).toContainText("MB");
});

test("an enum names its choices instead of the wire value", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const select = page.locator('select[name="ui.tray_language"]');
  await expect(select).toContainText("Follow the operating system");
  await expect(select.locator('option[value="auto"]')).toHaveCount(1);
});

test("the filter narrows the page to the rows that match", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const filter = page.locator("[data-settings-filter]");
  const counter = page.locator("[data-settings-count]");
  await expect(page.locator('[data-setting="server.addr"]')).toBeVisible();
  const total = await page.locator("[data-setting]").count();
  expect(total).toBeGreaterThan(2);
  await expect(counter).toHaveText(`${total} settings`);

  await filter.fill("tray");
  await expect(page.locator('[data-setting="ui.tray_language"]')).toBeVisible();
  await expect(page.locator('[data-setting="server.addr"]')).toBeHidden();
  await expect(page.locator('[data-setting-group="server_addresses"]')).toBeHidden();
  await expect(page.locator('[data-setting-group="server_desktop"]')).toBeVisible();
  await expect(
    page.locator('[data-setting-group="server_desktop"] [data-setting-group-count]'),
  ).toHaveText("2");
  await expect(counter).toHaveText(`2 of ${total} settings`);

  await filter.fill("no setting says this");
  await expect(page.locator("[data-settings-empty]")).toBeVisible();
  await expect(counter).toHaveText(`0 of ${total} settings`);

  await filter.fill("");
  await expect(page.locator('[data-setting="server.addr"]')).toBeVisible();
  await expect(page.locator("[data-settings-empty]")).toBeHidden();
  await expect(counter).toHaveText(`${total} settings`);
});

test("a regular account cannot reach the pages", async ({ page, browser }) => {
  const { createUserViaAdmin } = await import("../helpers/users");
  const email = await createUserViaAdmin(page, "settings-visitor", "settings-password-123");
  const visitor = await signInAs(browser, email, "settings-password-123");
  await visitor.page.goto("/sysadmin/settings/");
  await expect(visitor.page).toHaveURL(/\/libraries\//);
  await visitor.close();
});
