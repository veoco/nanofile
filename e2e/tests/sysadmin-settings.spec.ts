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
 * The panel is the page's own width: it starts at the same left edge as the
 * title and the filter above it. Inside it a setting is one of the app's list
 * rows, so its text is inset by the row's own padding rather than by the page.
 */
test("the panel lines up with the page header and the filter", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const title = (await page.locator("main .page-title").boundingBox())!;
  // The filter's own box: the input inside it starts after the search glyph, so
  // the control whose left edge is meant to line up is the label.
  const filter = (await page.locator("label.nf-find").boundingBox())!;
  const panel = (await page.locator("form.nf-list").boundingBox())!;
  for (const box of [filter, panel]) {
    expect(Math.abs(box.x - title.x)).toBeLessThan(1);
  }

  // A row's text starts one border plus one row padding inside the panel, and
  // its control column ends the same distance from the other edge.
  const row = page.locator('[data-setting="server.addr"]');
  const label = (await row.locator("label").boundingBox())!;
  expect(Math.round(label.x - panel.x)).toBe(15);
  const control = (await row.locator(".nf-srow-act").boundingBox())!;
  expect(Math.round(panel.x + panel.width - (control.x + control.width))).toBe(15);
});

/**
 * The group heading is the panel's own band: full width inside the panel, whose
 * border draws the sides the band used to have to draw itself, with the group's
 * count beside the label. It is also a separator — a hairline under it, and the
 * first group leaves the one above it to the panel's border.
 */
test("a group heading is a full-width band in the panel", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const band = page.locator('[data-setting-group="server_addresses"] .nf-sec');
  const panel = page.locator("form.nf-list");
  const box = (await band.boundingBox())!;
  const outer = (await panel.boundingBox())!;
  // The band spans the panel edge to edge, inside its border...
  expect(Math.abs(box.x - (outer.x + 1))).toBeLessThan(1);
  expect(Math.abs(box.width - (outer.width - 2))).toBeLessThan(1);
  // ...and the first group draws no top hairline of its own.
  await expect(band).toHaveCSS("border-bottom-width", "1px");
  await expect(band).toHaveCSS("border-top-width", "0px");

  // The label is inset by the band's own 14px.
  const heading = (await band.locator("h2").boundingBox())!;
  expect(Math.round(heading.x - box.x)).toBe(14);

  // 41px: an 11px label at `leading-none`, 14px of padding either side and the
  // two hairlines. The inherited 1.5 line height would make it 46.5px.
  expect(box.height).toBeLessThanOrEqual(42);
  const middle = (r: { y: number; height: number }) => r.y + r.height / 2;
  expect(Math.abs(middle(heading) - middle(box))).toBeLessThanOrEqual(1);
});

/**
 * A group heading's lower hairline closes the heading, so the first row under
 * it must not draw a second hairline right below it — including when the
 * filter has hidden rows, where the visible row is not the first child.
 */
test("a group heading is not doubled by its first row's border", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const rows = page.locator('[data-setting-group="server_addresses"] [data-setting]');
  await expect(rows.first()).toHaveCSS("border-top-width", "0px");
  await expect(rows.nth(1)).toHaveCSS("border-top-width", "1px");

  // Hide the first row: the row that moves up must not inherit its hairline.
  await page.locator("[data-settings-filter]").fill("bind port");
  await expect(rows.first()).toBeHidden();
  const moved = page.locator('[data-setting="server.port"]');
  await expect(moved).toBeVisible();
  await expect(moved).toHaveCSS("border-top-width", "0px");
});

/**
 * Restarting is not a save, so its button is a page-header action: on the
 * title's line and right of it, the way the user and email pages keep theirs.
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
    // The section bar is how an operator moves between areas.
    await expect(page.locator("main .nf-tabs a").first()).toBeVisible();
    // Every row carries an origin badge, and no label fell back to its key.
    const badges = await page.locator("main .badge").count();
    expect(badges, `${area || "general"} rendered no rows`).toBeGreaterThan(1);
    const body = (await page.locator("main").innerText()).trim();
    expect(body).not.toContain("setting.");
  }
});

/**
 * The Sandbox page is the one page whose subject is a measurement rather than a
 * setting: it says what this host actually gives, one protection at a time, and
 * warns when that is not the whole set.
 */
test("the sandbox page shows the measured grade and items", async ({ page }) => {
  await page.goto("/sysadmin/settings/sandbox/");
  // The verdict leads, the strip of four numbers follows it.
  await expect(page.locator("[data-sandbox-verdict-label]")).toBeVisible();
  await expect(page.locator("[data-sandbox-status] .field-label")).toHaveCount(4);
  await expect(page.locator("[data-sandbox-grade]")).toBeVisible();
  // One row per protection, in the order the report carries them.
  for (const item of ["limits", "files", "network", "process"]) {
    await expect(
      page.locator(`[data-sandbox-item="sandbox.item_${item}"]`),
    ).toBeVisible();
  }
  // The media worker is its own child with its own grade: the profile that
  // cannot have the process item says so on its own row.
  await expect(page.locator("[data-sandbox-media]")).toBeVisible();
  await expect(page.locator("[data-sandbox-media-state]")).toBeVisible();
  // The panel names a grade and an item state rather than a locale key.
  const body = (await page.locator("[data-sandbox-status]").innerText()).trim();
  expect(body).not.toContain("sandbox.grade_");
  expect(body).not.toContain("sandbox.item_");
});

/**
 * The page is a report, not a log: what each protection is and what its absence
 * opens is one row apiece, and the platform's own residuals — the paragraphs
 * about fork, `setpgid` and the loader, which used to run down the page — are
 * one disclosure away, closed by default.
 */
test("the sandbox page keeps its technical details in disclosures", async ({ page }) => {
  await page.goto("/sysadmin/settings/sandbox/");
  // The process note is the longest sentence on the page: it must not be in the
  // flow of it.
  const process = page.locator("[data-sandbox-media-process]");
  await expect(process).toBeHidden();

  await page.locator("[data-sandbox-media] details summary").click();
  await expect(process).toBeVisible();

  // A missing protection keeps its consequence in the row: hiding that would
  // leave the reader to infer it from a grey badge.
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
  await page.goto("/sysadmin/settings/server/");

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
    .locator('form[action="/sysadmin/settings/server/save/"]')
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

test("a row keeps its internal key out of the text", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const row = page.locator('[data-setting="server.max_upload_size_mb"]');
  // The key is a hook and a tooltip, not page furniture.
  const text = await row.innerText();
  expect(text).not.toContain("server.max_upload_size_mb");
  await expect(row.locator("label")).toHaveAttribute(
    "title",
    "server.max_upload_size_mb",
  );
  // One origin badge and at most one state badge.
  expect(await row.locator("span.badge").count()).toBeLessThanOrEqual(2);
});

test("a numeric row shows its unit beside the field", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const row = page.locator('[data-setting="server.max_upload_size_mb"]');
  await expect(row.locator('input[type="number"]')).toBeVisible();
  // The unit is rendered by the form, so the label does not repeat it.
  await expect(row.locator("label")).toHaveText("Max upload size");
  await expect(row).toContainText("MB");
});

test("an enum names its choices instead of the wire value", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const select = page.locator('select[name="ui.tray_language"]');
  await expect(select).toContainText("Follow the operating system");
  // What is submitted stays the value the server stores.
  await expect(select.locator('option[value="auto"]')).toHaveCount(1);
});

test("the filter narrows the page to the rows that match", async ({ page }) => {
  await page.goto("/sysadmin/settings/server/");
  const filter = page.locator("[data-settings-filter]");
  const counter = page.locator("[data-settings-count]");
  await expect(page.locator('[data-setting="server.addr"]')).toBeVisible();
  // Untouched, the counter is the page's own tally, rendered by the server.
  const total = await page.locator("[data-setting]").count();
  expect(total).toBeGreaterThan(2);
  await expect(counter).toHaveText(`${total} settings`);

  await filter.fill("tray");
  await expect(page.locator('[data-setting="ui.tray_language"]')).toBeVisible();
  await expect(page.locator('[data-setting="server.addr"]')).toBeHidden();
  // A heading with nothing left under it goes with its rows.
  await expect(page.locator('[data-setting-group="server_addresses"]')).toBeHidden();
  await expect(page.locator('[data-setting-group="server_desktop"]')).toBeVisible();
  // A heading counts what is left under it, not what it started with.
  await expect(
    page.locator('[data-setting-group="server_desktop"] [data-setting-group-count]'),
  ).toHaveText("2");
  // The page-wide counter says how much of the page is left.
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
