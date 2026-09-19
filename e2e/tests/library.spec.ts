import { test, expect } from "@playwright/test";
import { readState, createRepo, createEncryptedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
  // Seed a library so the list page is non-empty. Without this the spec only
  // passes when run as part of the full suite (other specs seed repos first);
  // running it in isolation would time out waiting for a list row.
  await createRepo(state.baseURL, state.adminToken, `seed-lib-${Date.now()}`);
});

async function openLibraries(page: import("@playwright/test").Page): Promise<void> {
  await page.goto("/libraries/");
  await page.waitForSelector('ul[role="list"] li');
}

// Row actions live behind the hover/focus-revealed "⋯" menu; the row itself is
// the navigation target, so the click has to go through the menu.
async function rowAction(
  page: import("@playwright/test").Page,
  row: import("@playwright/test").Locator,
  action: string,
): Promise<void> {
  await row.hover();
  await row.locator(".nf-more").click();
  await page.locator(`.nf-menu [data-action="${action}"]`).click();
}

async function createRepoByName(page: import("@playwright/test").Page, name: string): Promise<string> {
  const repoId = await createRepo(state.baseURL, state.adminToken, name);
  await page.reload();
  await page.waitForSelector('ul[role="list"] li');
  return repoId;
}

test("create a new library", async ({ page }) => {
  await openLibraries(page);
  await page.locator('button[data-action="show-create"]').click();
  await expect(page.locator("#create-overlay")).toBeVisible();
  await page.locator("#create-input").fill(`create-lib-${Date.now()}`);
  await page.locator('#create-overlay button[type="submit"]').click();
  await expect(page.locator("li").filter({ hasText: /create-lib-/ })).toBeVisible({ timeout: 15_000 });
});

test("edit a library name, description and history settings", async ({ page }) => {
  const name = `edit-lib-${Date.now()}`;
  await openLibraries(page);
  await createRepoByName(page, name);
  const li = page.locator("li").filter({ hasText: name });
  await rowAction(page, li, "show-edit");
  await expect(page.locator("#edit-overlay")).toBeVisible();
  await page.locator("#edit-name").fill("edited-lib");
  await page.locator("#edit-description").fill("edited description");
  await page.locator("#edit-history-limit").fill("10");
  await page.locator("#edit-history-ttl-days").fill("30");
  await page.locator('#edit-form button[type="submit"]').click();
  await expect(page.locator("li").filter({ hasText: "edited-lib" })).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("li").filter({ hasText: "edited description" })).toBeVisible();
});

test("the edit dialog links to API key management", async ({ page }) => {
  const name = `dav-lib-${Date.now()}`;
  await openLibraries(page);
  await createRepoByName(page, name);
  const li = page.locator("li").filter({ hasText: name });
  await rowAction(page, li, "show-edit");
  await expect(page.locator("#edit-overlay")).toBeVisible();
  // The dialog shows the per-library WebDAV URL and points at the key page
  // instead of managing keys inline.
  await expect(page.locator("#webdav-url")).toContainText("/dav/");
  await expect(
    page.locator('#edit-overlay a[href="/settings/api-keys/"]'),
  ).toBeVisible();
});

test("delete a library", async ({ page }) => {
  const name = `del-lib-${Date.now()}`;
  await openLibraries(page);
  await createRepoByName(page, name);
  const li = page.locator("li").filter({ hasText: name });
  await expect(li).toBeVisible();
  await rowAction(page, li, "delete-repo");
  await page.locator(".js-confirm-ok").click();
  await expect(li).toHaveCount(0, { timeout: 15_000 });
});

test("renders folder icon for normal and lock icon for encrypted libraries", async ({ page }) => {
  const normalName = `icon-normal-${Date.now()}`;
  const encName = `icon-enc-${Date.now()}`;
  await createRepo(state.baseURL, state.adminToken, normalName);
  await createEncryptedRepo(state.baseURL, state.adminToken, encName, "e2e-password");
  await openLibraries(page);

  // Normal library: one folder glyph in the leading icon cell.
  const normalRow = page.locator("li").filter({ hasText: normalName });
  await expect(normalRow.locator(".nf-lib-ic svg")).toHaveCount(1);
  await expect(normalRow.locator(".nf-lib-lock")).toHaveCount(0);

  // Encrypted library: the same folder glyph plus a lock beside the name (a
  // glyph rather than a badge, so colour stays reserved for state).
  const encRow = page.locator("li").filter({ hasText: encName });
  await expect(encRow.locator(".nf-lib-ic svg")).toHaveCount(1);
  await expect(encRow.locator(".nf-lib-lock")).toHaveCount(1);
});

test("the list filters and sorts without a round trip", async ({ page }) => {
  const name = `sort-lib-${Date.now()}`;
  await openLibraries(page);
  await createRepoByName(page, name);

  const rows = page.locator("#repo-list > li");
  const total = await rows.count();
  expect(total).toBeGreaterThan(1);

  // Filter: only rows whose name or description matches survive, and the count
  // label follows.
  await page.locator("#repo-filter").fill(name);
  await expect(page.locator("#repo-list > li:visible")).toHaveCount(1);
  await expect(page.locator("#repo-count")).toHaveText("1 library");

  // No match: the row list empties and the note names the query.
  await page.locator("#repo-filter").fill(`${name}-no-such-thing`);
  await expect(page.locator("#repo-list > li:visible")).toHaveCount(0);
  await expect(page.locator("#repo-no-match")).toContainText(`${name}-no-such-thing`);

  // Escape clears the field again.
  await page.locator("#repo-filter").press("Escape");
  await expect(page.locator("#repo-list > li:visible")).toHaveCount(total);

  // The default order is "last modified", newest first. Assert the order rather
  // than which library is first: repos created in the same second tie, and the
  // sort is stable, so the winner among equals is arbitrary.
  const mtimes = await rows.evaluateAll((els) =>
    els.map((el) => Number((el as HTMLElement).dataset.mtime)),
  );
  expect(mtimes).toEqual([...mtimes].sort((a, b) => b - a));

  await page.locator("#repo-sort-btn").click();
  // The popover has to actually be on top of the list: an ancestor with
  // `overflow` would clip it, and Playwright's scroll-into-view would hide that
  // from a plain click.
  await expect(page.locator("#repo-sort-pop")).toBeVisible();
  expect(
    await page.evaluate(() => {
      const pop = document.getElementById("repo-sort-pop")!;
      const r = pop.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return !!hit && !!hit.closest("#repo-sort-pop");
    }),
  ).toBe(true);
  await page.locator('#repo-sort-pop [data-sort="name"]').click();
  await expect(page.locator("#repo-sort-label")).toHaveText("Name");
  // Compare inside the page: the order has to match the browser's own collator,
  // which is the one the list used.
  const sorted = await rows.evaluateAll((els) => {
    const names = els.map((el) => (el as HTMLElement).dataset.name || "");
    const want = [...names].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
    return names.join("|") === want.join("|");
  });
  expect(sorted).toBe(true);
});

// The rail's filter is a navigation aid (switch library from inside one). On
// this page it would be a second box filtering the same set, so it stands down
// — but only here, and only because the page has a filter of its own.
test("the rail filter is absent here and the page filter is not", async ({ page }) => {
  await openLibraries(page);
  await expect(page.locator(".js-repo-filter")).toHaveCount(0);
  await expect(page.locator("#repo-filter")).toBeVisible();

  await page.goto("/starred/");
  await expect(page.locator(".js-repo-filter")).toBeVisible();
});

test("the row menu is reachable by keyboard", async ({ page }) => {
  await openLibraries(page);
  const row = page.locator("#repo-list > li").first();

  // Hidden at rest, revealed once the row has focus (not only on hover).
  await expect(row.locator(".nf-more")).toHaveCSS("opacity", "0");
  await row.locator(".nf-lib-nm").focus();
  await expect(row.locator(".nf-more")).toHaveCSS("opacity", "1");

  await row.locator(".nf-more").click();
  await expect(page.locator(".nf-menu")).toBeVisible();
  await expect(page.locator(".nf-menu .nf-menu-item").first()).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.locator(".nf-menu")).toHaveCount(0);
});

// The pre-paint preference guard and the translation table used to be inline
// <script> blocks. They are now an external bundle plus a JSON data block so
// the Content-Security-Policy can drop 'unsafe-inline' from script-src; these
// tests pin that they still work.
test("saved preferences are applied before paint", async ({ page }) => {
  await openLibraries(page);

  await page.evaluate(() => {
    localStorage.setItem("darkMode", "true");
    localStorage.setItem("fileViewMode", "gallery");
  });
  await page.reload();
  await page.waitForSelector('ul[role="list"] li');

  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(page.locator("html")).toHaveAttribute("data-view", "gallery");

  // Leave the browser state clean for later tests.
  await page.evaluate(() => {
    localStorage.removeItem("darkMode");
    localStorage.removeItem("fileViewMode");
  });
});

test("the translation table is published from the JSON data block", async ({ page }) => {
  await openLibraries(page);

  const dict = await page.evaluate(() => (window as unknown as { __T?: Record<string, string> }).__T);
  expect(dict).toBeTruthy();
  expect(dict!["app.name"]).toBeTruthy();

  // No inline executable script remains: every script is either external or a
  // non-executable JSON block.
  const inline = await page.evaluate(() =>
    Array.from(document.querySelectorAll("script"))
      .filter((s) => !s.src && s.type !== "application/json")
      .map((s) => s.textContent || ""),
  );
  expect(inline.join("").trim()).toBe("");
});
