import { test, expect } from "@playwright/test";
import { readState, createRepo, uploadFile } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

// Enough entries to scroll the list; the scroller is a little over 15 rows tall
// at the suite's viewport.
const ROWS = 30;
// A valid 1×1 PNG, so the gallery has one month group to head.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

test.beforeAll(async () => {
  state = readState();
  repoId = await createRepo(state.baseURL, state.adminToken, `sticky-headers-${Date.now()}`);
  for (let i = 0; i < ROWS; i++) {
    await uploadFile(
      state.baseURL,
      state.adminToken,
      repoId,
      "/",
      `row-${String(i).padStart(2, "0")}.txt`,
      `x${i}`,
    );
  }
});

// The sort/view toolbar and these column headers are sticky inside the same
// scroller (#nf-list-scroll). With every one of them at `top: 0` the header —
// later in the DOM, same z-index — pinned on top of the toolbar instead of
// under it, so scrolling a long list hid the view switcher and the sort
// buttons behind "NAME / SIZE / MODIFIED".
test("the column header pins under the toolbar, not over it", async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files/`);
  await page.waitForSelector(".js-entry-row");

  await page.locator("#nf-list-scroll").evaluate((el) => {
    el.scrollTop = 300;
  });
  await expect(page.locator(".nf-row-head")).toBeVisible();

  const boxes = await page.evaluate(() => {
    const box = (sel: string) => {
      const r = (document.querySelector(sel) as HTMLElement).getBoundingClientRect();
      return { top: Math.round(r.top), bottom: Math.round(r.bottom), h: Math.round(r.height) };
    };
    return { toolbar: box(".js-sort-bar"), head: box(".nf-row-head") };
  });
  expect(boxes.head.top).toBe(boxes.toolbar.top + boxes.toolbar.h);
  expect(boxes.head.bottom).toBeGreaterThan(boxes.toolbar.bottom);

  // The controls the header used to cover are reachable: the sort click is
  // intercepted by the header if this regresses, and the order has to change.
  const firstFile = page.locator('.js-file-list-view .js-entry-row[data-type="file"]').first();
  const before = await firstFile.getAttribute("data-name");
  await page.locator('.js-sort-btn[data-sort="name"]').click();
  await expect.poll(() => firstFile.getAttribute("data-name")).not.toBe(before);
});

// The gallery's month header is sticky in that same scroller and shared the
// same `top: 0`; it has to sit at the toolbar's height too.
test("the gallery month header pins at the toolbar's height", async ({ page }) => {
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "photo.png", PNG);

  await page.goto(`/libraries/${repoId}/files/`);
  await page.waitForSelector(".js-entry-row");
  await page.locator(".js-view-gallery").click();
  await expect(page.locator(".nf-gal-head")).toBeVisible();

  const offsets = await page.evaluate(() => {
    const toolbar = document.querySelector(".js-sort-bar") as HTMLElement;
    const head = document.querySelector(".nf-gal-head") as HTMLElement;
    return {
      headTop: getComputedStyle(head).top,
      toolbarHeight: `${Math.round(toolbar.getBoundingClientRect().height)}px`,
    };
  });
  expect(offsets.headTop).toBe(offsets.toolbarHeight);
});
