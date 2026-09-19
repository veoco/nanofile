import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";
import { bmpFixture } from "../helpers/image";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "views-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("switches between list, grid, and gallery views", async ({ page }) => {
  await expect(page.locator(".js-file-list-view")).toBeVisible();
  await page.locator(".js-view-grid").click();
  await expect(page.locator(".js-file-grid-view")).toBeVisible();
  await page.locator(".js-view-gallery").click();
  await expect(page.locator(".js-gallery-view")).toBeVisible();
  await page.locator(".js-view-list").click();
  await expect(page.locator(".js-file-list-view")).toBeVisible();
});

test("view mode persists across reload", async ({ page }) => {
  await page.locator(".js-view-grid").click();
  await expect(page.locator(".js-file-grid-view")).toBeVisible();
  await page.reload();
  await expect(page.locator(".js-file-grid-view")).toBeVisible();
});

// ─── Grid tiles are real rows, not decoration ───────────────────────────────
// The grid and gallery render the same `.js-entry-row` markup as the list,
// which is what lets the shared "⋯" menu and the star toggle work there with no
// new JavaScript at all. These tests pin that contract down.

test("a grid tile's ⋯ menu drives the same actions as a list row", async ({ page }) => {
  await page.locator(".js-view-grid").click();
  await expect(page.locator(".js-file-grid-view")).toBeVisible();

  const tileMenu = page.locator(
    '.js-file-grid-view .js-entry-row[data-name="charlie.txt"] .nf-more',
  );
  await expect(tileMenu).toHaveCount(1);
  await tileMenu.click();

  const menu = page.locator(".nf-menu");
  await expect(menu).toBeVisible();
  await menu.locator(".js-rename-btn").click();

  await page.locator("#rename-input").fill("charlie2.txt");
  await page.locator('#rename-dialog-form button[type="submit"]').click();
  await expect(
    page.locator('.js-file-grid-view .js-entry-row[data-name="charlie2.txt"]'),
  ).toBeVisible({ timeout: 15_000 });
});

test("starring from a grid tile persists and keeps its chips visible", async ({ page }) => {
  await page.locator(".js-view-grid").click();
  const tile = page.locator(
    '.js-file-grid-view .js-entry-row[data-name="delta.txt"]',
  );
  // Unhovered and unstarred: the action chips stay out of the way.
  await expect(tile.locator(".nf-tile-tools")).toHaveCSS("opacity", "0");

  await tile.locator("[data-toggle-star]").click();
  await expect(tile).toHaveClass(/starred/);
  await expect(tile).toHaveAttribute("data-starred", "true");
  // `.starred` is what keeps the chips (the star included) on screen for an
  // item you are not pointing at.
  await expect(tile.locator(".nf-tile-tools")).toHaveCSS("opacity", "1");

  await page.reload();
  await page.locator(".js-view-grid").click();
  await expect(
    page.locator('.js-file-grid-view .js-entry-row[data-name="delta.txt"]'),
  ).toHaveClass(/starred/);
});

test("selecting a grid tile draws an inset ring on the media box", async ({ page }) => {
  await page.locator(".js-view-grid").click();
  const tile = page.locator(
    '.js-file-grid-view .js-entry-row[data-name="alpha.txt"]',
  );
  await tile.locator(".nf-tile-media").click();
  await expect(tile).toHaveClass(/selected/);

  // The old affordance was a 12% grey wash over the whole card, which was
  // indistinguishable from hover. The ring now sits on the media box and is
  // inset, so it cannot bleed over a neighbouring tile.
  await expect(tile).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  const media = tile.locator(".nf-tile-media");
  await expect(media).toHaveCSS("outline-style", "solid");
  await expect(media).toHaveCSS("outline-width", "2px");
  await expect(media).toHaveCSS("outline-offset", "-2px");

  // …and the shared selection plumbing still opens the details drawer.
  await expect(page.locator("#nf-details")).toBeVisible();
});

// ─── Gallery: what a contact sheet is allowed to contain ────────────────────

test("gallery keeps only pictures, names every tile, and counts the month", async ({
  page,
}) => {
  // A valid 1×1 PNG so the tile exercises the thumbnail path, plus an audio
  // file and documents that must NOT appear in the contact sheet.
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    "base64",
  );
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "photo.png", png);
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "song.mp3", Buffer.alloc(64, 9));

  await page.reload();
  await page.waitForSelector(".js-entry-row");
  await page.locator(".js-view-gallery").click();

  const tiles = page.locator(".js-gallery-view .js-entry-row");
  await expect(tiles).toHaveCount(1);
  await expect(tiles.first()).toHaveAttribute("data-name", "photo.png");
  // Audio has no picture to show; it stays reachable in the list and grid.
  await expect(
    page.locator('.js-gallery-view .js-entry-row[data-name="song.mp3"]'),
  ).toHaveCount(0);

  // The caption belongs to the tile, it is not a hover overlay.
  const caption = tiles.first().locator(".nf-gal-cap");
  await expect(caption).toBeVisible();
  await expect(caption).toHaveText("photo.png");
  await expect(caption).toHaveCSS("opacity", "1");

  await expect(page.locator(".js-gallery-view .nf-gal-head .nf-gal-n")).toHaveText("1");
  await expect(tiles.first().locator(".nf-more")).toHaveCount(1);
});

// ─── Thumbnails: one size, one container ────────────────────────────────────
// The tile used to be 2.4x upscaled on a 2x display: the URL asked for
// `size=256` and the server answered in PNG, so the size could not simply be
// raised — a 640px PNG measured 0.5 MB against 70 KB for the same photo as
// JPEG. These are the two halves of that one decision.

test("grid tiles fetch a 2x-sized JPEG, and the gallery shares the URL", async ({ page }) => {
  // 1200x900 (so the 640 thumbnail is a real downscale), opaque, and dithered
  // enough that PNG and JPEG differ by an order of magnitude on it.
  await uploadFile(
    state.baseURL,
    state.adminToken,
    repoId,
    "/",
    "wide-scene.bmp",
    bmpFixture(1200, 900),
  );

  await page.reload();
  await page.locator(".js-view-grid").click();

  const tile = page.locator('.js-file-grid-view .js-entry-row[data-name="wide-scene.bmp"]');
  const img = tile.locator("img");
  await img.scrollIntoViewIfNeeded();
  // Lazy loading means the bitmap only arrives once the tile is on screen.
  await expect
    .poll(async () => img.evaluate((el: HTMLImageElement) => el.naturalWidth))
    .toBeGreaterThan(0);

  const src = await img.getAttribute("src");
  expect(src).toContain("size=640");

  // The tile media box is 227 CSS px wide — 454 device px on a 2x display. The
  // thumbnail must be at least that wide or the browser upscales it.
  const natural = await img.evaluate((el: HTMLImageElement) => [
    el.naturalWidth,
    el.naturalHeight,
  ]);
  expect(natural[0]).toBeGreaterThanOrEqual(454);

  const resp = await page.request.get(src!);
  expect(resp.headers()["content-type"]).toBe("image/jpeg");
  // A PNG of this scene at 640 measured several hundred KB.
  expect((await resp.body()).length).toBeLessThan(150_000);

  // Grid and gallery deliberately share one cached size: switching views was
  // measured to hit the browser cache, not the network.
  await page.locator(".js-view-gallery").click();
  const gallerySrc = await page
    .locator('.js-gallery-view .js-entry-row[data-name="wide-scene.bmp"] img')
    .getAttribute("src");
  expect(gallerySrc).toBe(src);
});

