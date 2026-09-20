import { test, expect } from "@playwright/test";
import { readState, createRepo, uploadFile } from "../helpers/api";
import { backdateFileMtimes } from "../helpers/mtime";

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

// `top` only fixes where a sticky header *rests*. A sticky header is also
// clamped by the bottom edge of the group it belongs to, so at every month
// boundary the outgoing header slides up across the toolbar's band before it
// leaves. Both used to be at z-index 10, and the header — later in the DOM — won
// that tie, dragging its label and its rule over the view switcher mid-scroll.
test("a month header leaving its group does not paint over the toolbar", async ({ page }) => {
  const galleryRepoId = await createRepo(
    state.baseURL,
    state.adminToken,
    `sticky-gallery-${Date.now()}`,
  );
  for (const prefix of ["sh1-", "sh2-"]) {
    for (let i = 0; i < 20; i++) {
      await uploadFile(
        state.baseURL,
        state.adminToken,
        galleryRepoId,
        "/",
        `${prefix}${String(i).padStart(2, "0")}.png`,
        PNG,
      );
    }
  }
  // Two months, so the gallery has a boundary for a header to leave.
  expect(backdateFileMtimes("sh1-", Date.UTC(2025, 2, 15) / 1000)).toBeGreaterThan(0);

  await page.goto(`/libraries/${galleryRepoId}/files/`);
  await page.waitForSelector(".js-entry-row");
  await page.locator(".js-view-gallery").click();
  await expect(page.locator(".nf-gal-head")).toHaveCount(2);

  // Walk the whole scroller. Wherever a month header crosses the toolbar's band,
  // the toolbar has to be what the pointer hits — on the view buttons and
  // anywhere else along the bar.
  const sweep = await page.evaluate(() => {
    const scroller = document.getElementById("nf-list-scroll") as HTMLElement;
    const bar = document.querySelector(".js-sort-bar") as HTMLElement;
    const buttons = Array.from(bar.querySelectorAll(".seg button"));
    const heads = Array.from(document.querySelectorAll(".nf-gal-head"));
    const escapes: string[] = [];
    let overlaps = 0;
    for (let top = 0; top <= scroller.scrollHeight; top += 20) {
      scroller.scrollTop = top;
      const b = bar.getBoundingClientRect();
      for (const head of heads) {
        const h = head.getBoundingClientRect();
        const y1 = Math.max(b.top, h.top);
        const y2 = Math.min(b.bottom, h.bottom);
        if (y2 <= y1) continue;
        overlaps += 1;
        const y = Math.round((y1 + y2) / 2);
        const probes = [
          ...buttons.map((el) => {
            const r = el.getBoundingClientRect();
            return {
              x: Math.round(r.left + r.width / 2),
              label: el.getAttribute("title") ?? "view button",
            };
          }),
          { x: Math.round(b.left + 60), label: "bar" },
        ];
        for (const { x, label } of probes) {
          const hit = document.elementFromPoint(x, y);
          if (!hit || !bar.contains(hit)) {
            escapes.push(`scrollTop=${top} ${label}@${x} y=${y} hit=${hit?.className ?? "null"}`);
          }
        }
      }
    }
    return { overlaps, escapes };
  });
  // The collision has to have actually happened, or the test proves nothing.
  expect(sweep.overlaps).toBeGreaterThan(0);
  expect(sweep.escapes).toEqual([]);
});
