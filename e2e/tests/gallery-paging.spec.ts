import { test, expect } from "@playwright/test";
import { readState, createRepo, uploadFile } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

// The browser fetches 200 entries at a time, so the gallery needs a page
// boundary to cross — and a boundary inside a month, which is the normal case
// for a photo library.
const PAGE = 200;
const MEDIA = 201;

// A valid 1×1 PNG: real media as far as the gallery's entry filter is
// concerned, and cheap enough to upload 201 of them.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

test.beforeAll(async () => {
  state = readState();
  repoId = await createRepo(state.baseURL, state.adminToken, `gallery-paging-${Date.now()}`);
  for (let i = 0; i < MEDIA; i++) {
    await uploadFile(
      state.baseURL,
      state.adminToken,
      repoId,
      "/",
      `p-${String(i).padStart(3, "0")}.png`,
      PNG,
    );
  }
});

// The gallery appends month groups rather than rows. It used to select them by
// `.gallery-month-group`, the class the gallery had before the UI rebuild
// renamed it, so the fetch came back 200 and appended nothing: scrolling to the
// bottom advanced the page counter, hid the bar and never showed another photo.
test("the gallery's bottom auto-load appends the next page", async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files/`);
  await page.waitForSelector(".js-entry-row");
  await page.locator(".js-view-gallery").click();
  await expect(page.locator(".js-gallery-view")).toBeVisible();

  const tiles = page.locator(".js-gallery-view .js-entry-row");
  await expect(tiles).toHaveCount(PAGE);

  // Reaching the bottom has to fetch page 2 by itself — there is no button to
  // press in this view.
  await page.locator("#nf-list-scroll").evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  await expect(tiles).toHaveCount(MEDIA);

  // Page 2 is the tail of the same month, so its photos belong to the group
  // already on screen: one heading carrying the whole count, not a second
  // identical heading with a remainder.
  await expect(page.locator(".js-gallery-view .nf-gal-group")).toHaveCount(1);
  await expect(page.locator(".js-gallery-view .nf-gal-n")).toHaveText(String(MEDIA));
});
