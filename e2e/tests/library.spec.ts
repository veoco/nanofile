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

// The list's own name comparison: lowercased code points, the one ui/repos.rs
// and repos.js share. The browser's collator weights punctuation differently,
// so `localeCompare` would disagree with the rendered order.
function byName(a: string, b: string): number {
  const x = a.toLowerCase();
  const y = b.toLowerCase();
  return x < y ? -1 : x > y ? 1 : 0;
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

test("the list spans the same box as its toolbar", async ({ page }) => {
  await openLibraries(page);

// Full-bleed, like the file list inside a library: the header band and the row rules reach both
// edges, and rows carry only the toolbar's inset.
  const box = await page.evaluate(() => {
    const rect = (sel: string) => {
      const r = document.querySelector(sel)!.getBoundingClientRect();
      return { left: Math.round(r.left), right: Math.round(window.innerWidth - r.right) };
    };
    return {
      toolbar: rect(".nf-toolbar"),
      head: rect(".nf-lib-head"),
      row: rect("#repo-list > li"),
    };
  });
  expect(box.row).toEqual(box.toolbar);
  expect(box.head).toEqual(box.toolbar);
});

// The header band is one more row of the grid, so its hairline is the rows' own `--color-line`;
// the two are asserted to be the same colour rather than a named token.
test("the column header's hairline matches the rows'", async ({ page }) => {
// A second library: the last row's `border-bottom: 0` drops its colour to `currentColor`, so
// comparing against it would assert the wrong thing.
  await createRepo(state.baseURL, state.adminToken, `hairline-lib-${Date.now()}`);
  await openLibraries(page);

  const colors = await page.evaluate(() => {
    const bottom = (sel: string) =>
      getComputedStyle(document.querySelector(sel)!).borderBottomColor;
    return { head: bottom(".nf-lib-head"), row: bottom("#repo-list > li:not(:last-child)") };
  });
  expect(colors.row).not.toBe("rgba(0, 0, 0, 0)");
  expect(colors.head).toBe(colors.row);
});

// The list used to sort only in the browser, so rows jumped once the bundle ran; the default
// order is the server's now, and nothing moves on load.
test("the default order is already the server-rendered one", async ({ page }) => {
// A name that sorts ahead of `seed-lib-…`, so the server must reorder, not emit membership order.
  await createRepo(state.baseURL, state.adminToken, `aaa-order-${Date.now()}`);

  const html = await (await page.request.get("/libraries/")).text();
  const ssr = [...html.matchAll(/data-name="([^"]+)"/g)].map((m) => m[1]);
  expect(ssr.length).toBeGreaterThan(1);
  // The rendered order is the list's own name comparison, not membership order.
  expect(ssr).toEqual([...ssr].sort(byName));

  await openLibraries(page);
  const dom = await page
    .locator("#repo-list > li")
    .evaluateAll((els) => els.map((el) => (el as HTMLElement).dataset.name));
  expect(dom).toEqual(ssr);
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

// This is the one underlined search control left: only the bottom edge draws a hairline, and the
// top edge drawing none is the exception this pins.
  const toolbarFilter = page.locator(".nf-toolbar label.nf-find");
  await expect(toolbarFilter).toHaveCount(1);
  const edges = await toolbarFilter.evaluate((el) => {
    const s = getComputedStyle(el);
    return { top: s.borderTopWidth, bottom: s.borderBottomWidth };
  });
  expect(edges).toEqual({ top: "0px", bottom: "1px" });

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

  // The control is the file list's: one flat text button per field, no popover,
  // and the name-ascending default is the button marked active.
  const namesInOrder = () =>
    rows.evaluateAll((els) => els.map((el) => (el as HTMLElement).dataset.name || ""));
  await expect(page.locator("#repo-sort-pop")).toHaveCount(0);
  await expect(page.locator(".js-repo-sort-btn")).toHaveCount(3);
  const nameBtn = page.locator('.js-repo-sort-btn[data-sort="name"]');
  await expect(nameBtn).toHaveClass(/\bon\b/);

  const ascending = await namesInOrder();
  expect(ascending).toEqual([...ascending].sort(byName));

  // Clicking the active field flips the direction.
  await nameBtn.click();
  const descending = await namesInOrder();
  expect(descending).toEqual([...descending].sort((a, b) => byName(b, a)));
});

// The rail filter narrows the rail's tree and renders on every page, /libraries/ included; its
// query must not touch the page's table, which has its own filter.
test("the rail filter is on every page and filters the rail", async ({ page }) => {
  await openLibraries(page);
  const railFilter = page.locator(".js-repo-filter");
  const pageFilter = page.locator("#repo-filter");
  await expect(railFilter).toBeVisible();
  await expect(pageFilter).toBeVisible();
// Same control, same copy: both boxes narrow the same set.
  await expect(railFilter).toHaveAttribute(
    "placeholder",
    (await pageFilter.getAttribute("placeholder")) ?? "",
  );

  const name = `rail-filter-lib-${Date.now()}`;
  await createRepoByName(page, name);

  const pageRows = page.locator("#repo-list > li:visible");
  const pageTotal = await pageRows.count();
  expect(pageTotal).toBeGreaterThan(1);

  await railFilter.fill(name);
  await expect(page.locator(".js-repo-item:visible")).toHaveCount(1);
  await expect(pageRows).toHaveCount(pageTotal);

  await page.goto("/starred/");
  await expect(page.locator(".js-repo-filter")).toBeVisible();

// Inside a library the rail filter is the only way to switch libraries, so it must come back:
// the file browser shares `active_page == "repos"` with the list page.
  const repoId = await createRepo(state.baseURL, state.adminToken, `rail-filter-browse-${Date.now()}`);
  await page.goto(`/libraries/${repoId}/files/`);
  await expect(page.locator(".js-repo-filter")).toBeVisible();
});

test("the row menu is reachable by keyboard", async ({ page }) => {
  await openLibraries(page);
  const row = page.locator("#repo-list > li").first();

  await expect(row.locator(".nf-more")).toHaveCSS("opacity", "0");
  await row.locator(".nf-lib-nm").focus();
  await expect(row.locator(".nf-more")).toHaveCSS("opacity", "1");

  await row.locator(".nf-more").click();
  await expect(page.locator(".nf-menu")).toBeVisible();
  await expect(page.locator(".nf-menu .nf-menu-item").first()).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.locator(".nf-menu")).toHaveCount(0);
});

// The preference guard and translation table are external now so CSP can drop `unsafe-inline`;
// these pin that both still work.
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
