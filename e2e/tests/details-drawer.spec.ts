import { test, expect, type Page } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";
import { expandDetails } from "../helpers/details";

// The details panel moved from a 300px right-hand column to an overlay pinned to
// the bottom of the file-manager column. The whole point of an overlay is that
// opening it reflows nothing, so these tests measure geometry rather than
// eyeballing screenshots: the list must keep its exact height and top edge, and
// the sidebar must not move at all, in every drawer state.

let state: ReturnType<typeof readState>;
let repoId: string;

const ROW_COUNT = 30;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "drawer-repo");
  // Enough rows to scroll: the "last row clears the drawer" assertion needs a
  // list taller than the viewport.
  for (let i = 0; i < ROW_COUNT; i++) {
    await uploadFile(
      state.baseURL,
      state.adminToken,
      repoId,
      "/",
      `filler-${String(i).padStart(3, "0")}.txt`,
      `content ${i}\n`,
    );
  }
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

async function geometry(page: Page) {
  return page.evaluate(() => {
    const box = (sel: string) => {
      const el = document.querySelector(sel);
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height), bottom: Math.round(r.bottom) };
    };
    const scroller = document.getElementById("nf-list-scroll");
    const drawer = document.getElementById("nf-details");
    return {
      list: box(".js-file-list-view"),
      side: box(".js-left-panel"),
      drawer: box("#nf-details"),
      content: box("#nf-content"),
      drawerVisible: !!drawer && getComputedStyle(drawer).visibility === "visible",
      paddingBottom: scroller ? parseFloat(getComputedStyle(scroller).paddingBottom) : -1,
    };
  });
}

/**
 * The drawer slides in over 0.22s, and the class flips synchronously — so a
 * measurement taken right after the click catches it mid-flight. Wait for the
 * transform to actually settle (translateY ≈ 0) instead of sleeping.
 */
async function settleDrawer(page: Page) {
  await page.waitForFunction(() => {
    const el = document.getElementById("nf-details");
    if (!el) return false;
    const t = getComputedStyle(el).transform;
    if (!t || t === "none") return true;
    const m = new DOMMatrixReadOnly(t);
    return Math.abs(m.m42) < 0.5;
  });
}

async function select(page: Page, name: string) {
  await page
    .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] > div:first-child`)
    .click();
  await expect(page.locator("#nf-details")).toHaveClass(/\bopen\b/);
  await settleDrawer(page);
}

test("the drawer is closed until something is selected", async ({ page }) => {
  const g = await geometry(page);
  expect(g.drawerVisible).toBe(false);
  expect(g.paddingBottom).toBe(0);
  // An unopened overlay must not reserve any space.
  expect(g.drawer?.h ?? 0).toBe(0);
});

test("selecting a row opens the drawer without reflowing anything", async ({ page }) => {
  const closed = await geometry(page);
  await select(page, "alpha.txt");
  const open = await geometry(page);

  expect(open.drawerVisible).toBe(true);
  // The list keeps its exact height and top edge — no reflow.
  expect(open.list).toEqual(closed.list);
  // The sidebar is not even in this column, so it cannot move.
  expect(open.side).toEqual(closed.side);
  // Only the scrollable extent grows, by exactly the drawer's height.
  expect(open.paddingBottom).toBeGreaterThan(0);
  expect(Math.abs(open.paddingBottom - (open.drawer?.h ?? 0))).toBeLessThanOrEqual(1);
});

test("the drawer is flush with the bottom of the file-manager column", async ({ page }) => {
  await select(page, "alpha.txt");
  const g = await geometry(page);
  const viewportBottom = await page.evaluate(() => window.innerHeight);

  expect(g.drawer).not.toBeNull();
  expect(Math.abs((g.drawer?.bottom ?? 0) - viewportBottom)).toBeLessThanOrEqual(1);
  // Scoped to the content column, not full-bleed under the sidebar.
  expect(g.drawer?.x).toBe(g.content?.x);
  if (g.side) {
    expect(g.drawer?.x ?? 0).toBeGreaterThanOrEqual(g.side.x + g.side.w - 1);
  }
});

test("expanding the rich tier still reflows nothing", async ({ page }) => {
  const closed = await geometry(page);
  await select(page, "alpha.txt");
  const single = await geometry(page);
  await expandDetails(page);
  const expanded = await geometry(page);

  expect(expanded.list).toEqual(closed.list);
  expect(expanded.side).toEqual(closed.side);
  // Expanding grows the drawer, and the reserved space follows it.
  expect(expanded.drawer?.h ?? 0).toBeGreaterThan(single.drawer?.h ?? 0);
  expect(Math.abs(expanded.paddingBottom - (expanded.drawer?.h ?? 0))).toBeLessThanOrEqual(1);
});

test("a multi selection reflows nothing either", async ({ page }) => {
  const closed = await geometry(page);
  await select(page, "alpha.txt");
  await page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="bravo.txt"] > div:first-child')
    .click({ modifiers: ["Control"] });
  const multi = await geometry(page);

  expect(multi.list).toEqual(closed.list);
  expect(multi.side).toEqual(closed.side);
  await expect(page.locator(".js-rp-multi-count")).toContainText("2");
});

test("the last row can be scrolled clear of the drawer", async ({ page }) => {
  const lastName = `filler-${String(ROW_COUNT - 1).padStart(3, "0")}.txt`;
  await select(page, lastName);
  const g = await geometry(page);
  await page.evaluate(() => {
    const s = document.getElementById("nf-list-scroll");
    if (s) s.scrollTop = s.scrollHeight;
  });
  const lastBottom = await page.evaluate((name) => {
    const row = document.querySelector(`.js-entry-row[data-name="${name}"]`);
    return row ? Math.round(row.getBoundingClientRect().bottom) : null;
  }, lastName);
  expect(lastBottom).not.toBeNull();
  expect(lastBottom as number).toBeLessThanOrEqual((g.drawer?.y ?? 0) + 1);
});

test("every row exposes its action button", async ({ page }) => {
  const counts = await page.evaluate(() => {
    const all = Array.from(document.querySelectorAll(".js-file-list-view .nf-more"));
    return { total: all.length, visible: all.filter((b) => b.getBoundingClientRect().width > 0).length };
  });
  expect(counts.visible).toBe(counts.total);
  expect(counts.total).toBeGreaterThan(0);
});

test("Escape and the close button both dismiss the drawer", async ({ page }) => {
  await select(page, "alpha.txt");
  await page.keyboard.press("Escape");
  await expect(page.locator("#nf-details")).not.toHaveClass(/\bopen\b/);
  expect((await geometry(page)).paddingBottom).toBe(0);

  await select(page, "bravo.txt");
  await page.locator(".js-rp-content .js-deselect-all").click();
  await expect(page.locator("#nf-details")).not.toHaveClass(/\bopen\b/);
  await expect(page.locator(".js-entry-row.selected")).toHaveCount(0);
});
