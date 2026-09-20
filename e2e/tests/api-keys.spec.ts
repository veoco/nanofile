import { test, expect } from "@playwright/test";
import { readState, createRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  // The scope picker lists the account's libraries; seed one so the page is
  // never empty when this spec runs in isolation.
  repoId = await createRepo(state.baseURL, state.adminToken, `key-lib-${Date.now()}`);
});

test.beforeEach(async ({ page }) => {
  await page.goto("/settings/api-keys/");
});

test("the settings index links to the key page", async ({ page }) => {
  await page.goto("/settings/");
  await page.locator('a[href="/settings/api-keys/"]').first().click();
  await expect(page).toHaveURL(/\/settings\/api-keys\/$/);
  await expect(page.locator("h1")).toContainText("API Keys");
});

test("create a key, see it once, then revoke it", async ({ page }) => {
  const name = `e2e-key-${Date.now()}`;
  await page.locator("#create-key-name").fill(name);
  // A read capability and a write capability, plus this spec's own library
  // (named by id so an earlier spec's library cannot take the slot).
  await page.locator('#create-key-form input[name="cap__file.read"]').check();
  await page.locator('#create-key-form input[name="cap__file.write"]').check();
  const repoSelect = page.locator(`#create-key-form select[name="repo__${repoId}"]`);
  await repoSelect.selectOption("rw");
  await page.locator("#create-key-expiry").selectOption("30");
  await page.locator('#create-key-form button[type="submit"]').click();

  // The plaintext is shown exactly once.
  await expect(page.locator("#new-key-value")).toBeVisible();
  const secret = await page.locator("#new-key-value").inputValue();
  expect(secret).toHaveLength(40);

  // And the key is listed.
  const card = page.locator(".nf-xrow").filter({ hasText: name });
  await expect(card).toBeVisible();
  await expect(card).toContainText("file.write");

  // The row's dates are localized by the browser (`data-ts`), not pre-formatted
  // in UTC by the server. A freshly minted key has never been used yet.
  const value = (label: string) =>
    card.locator(".nf-facts .f").filter({ hasText: label }).locator(".v");
  await expect(value("Created")).toHaveAttribute("data-ts", /^\d+$/);
  await expect(value("Created")).toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  await expect(value("Expires")).toHaveAttribute("data-ts", /^\d+$/);
  await expect(value("Expires")).toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  await expect(value("Last used")).toHaveText("Never");

  // One request with the key stamps its last-used time, which then renders in
  // the same local form as the other two dates. The key holds `file.read` for
  // this library, so list its files.
  const used = await page.request.get(
    `${state.baseURL}/api2/repos/${repoId}/dir/?p=/`,
    { headers: { authorization: `Bearer ${secret}` } },
  );
  expect(used.ok()).toBeTruthy();
  await page.reload();
  await expect(value("Last used")).toHaveAttribute("data-ts", /^\d+$/);
  await expect(value("Last used")).toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);

  // The reload also drops the one-time secret: the key stays, the plaintext
  // does not, and no second key appears (the create form redirects, so a
  // refresh re-reads the list).
  await expect(page.locator("#new-key-value")).toHaveCount(0);
  await expect(page.locator(".nf-xrow").filter({ hasText: name })).toHaveCount(1);

  // Revoke through the app's own confirm dialog, and it disappears.
  await page
    .locator(".nf-xrow")
    .filter({ hasText: name })
    .locator('form[action$="/revoke/"] button[type="submit"]')
    .click();
  await page.locator(".js-confirm-ok").click();
  await expect(page.locator(".nf-xrow").filter({ hasText: name })).toHaveCount(0);
});

test("a preset ticks the matching capabilities", async ({ page }) => {
  await page.locator("#apikey-preset").selectOption("webdav_ro");
  const box = (id: string) => page.locator(`#create-key-form input[name="cap__${id}"]`);
  // A WebDAV preset stays inside the WebDAV surface: it must not hand the REST
  // API of the bound libraries to a key created to mount a filesystem.
  await expect(box("webdav.read")).toBeChecked();
  await expect(box("webdav.write")).not.toBeChecked();
  await expect(box("file.read")).not.toBeChecked();
  await expect(box("file.write")).not.toBeChecked();
  await expect(box("library.read")).not.toBeChecked();

  // Switching to a write preset also selects the write side.
  await page.locator("#apikey-preset").selectOption("webdav_rw");
  await expect(box("webdav.write")).toBeChecked();
  await expect(box("file.write")).not.toBeChecked();

  // The search family splits manual index writes from a whole-repo reindex.
  await page.locator("#apikey-preset").selectOption("full");
  await expect(box("search.write")).toBeChecked();
  await expect(box("search.reindex")).toBeChecked();
});

test("the picker says what a capability grants and how broad a preset is", async ({ page }) => {
  // Each capability names its enforcement points, so "which endpoint does this
  // buy?" is answered in the page rather than by documentation.
  const label = (id: string) =>
    page.locator(`#create-key-form label:has(input[name="cap__${id}"])`);
  await expect(label("search.write")).toHaveAttribute("title", "POST /api2/index-file-text/");
  await expect(label("search.read")).toHaveAttribute("title", /GET \/api2\/search\//);
  await expect(label("webdav.write")).toHaveAttribute("title", /\/dav\/\{repo_id\}/);

  // A preset's label carries its size, so an over-broad one is visible first.
  await expect(page.locator('#apikey-preset option[value="webdav_ro"]')).toHaveText(
    "WebDAV (read only) (1)",
  );
});

test("a key without capabilities is rejected without leaking a secret", async ({ page }) => {
  await page.locator("#create-key-name").fill(`e2e-empty-${Date.now()}`);
  await page.locator("#create-key-expiry").selectOption("30");
  await page.locator('#create-key-form button[type="submit"]').click();
  await expect(page.locator("#new-key-value")).toHaveCount(0);
  await expect(page.getByText("at least one capability")).toBeVisible();
});
