import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";
import { pngFixture } from "../helpers/image";
import { tinyMp4 } from "../helpers/video";

let state: ReturnType<typeof readState>;
let repoId: string;

const PNG_1x1 = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

// Minimal 8kHz mono 16-bit PCM WAV (44-byte header + 1 silent sample).
function minimalWav(): Buffer {
  const b = Buffer.alloc(46);
  b.write("RIFF", 0);
  b.writeUInt32LE(42, 4);
  b.write("WAVE", 8);
  b.write("fmt ", 12);
  b.writeUInt32LE(16, 16);
  b.writeUInt16LE(1, 20);
  b.writeUInt16LE(1, 22);
  b.writeUInt32LE(8000, 24);
  b.writeUInt32LE(16000, 28);
  b.writeUInt16LE(2, 32);
  b.writeUInt16LE(16, 34);
  b.write("data", 36);
  b.writeUInt32LE(2, 40);
  b.writeUInt16LE(0, 44);
  return b;
}

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "preview-repo");
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "photo.png", PNG_1x1);
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "song.wav", minimalWav());
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "clip.mp4", Buffer.from("not really a video"));
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "plays.mp4", tinyMp4());
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "note.xyz", "unrecognized\n");
  // Bigger than the preview stage at the suite's viewport, and long enough that
  // the code block has to scroll: the two fixtures the frame assertions need.
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "big.png", pngFixture(1600, 1200));
  await uploadFile(
    state.baseURL,
    state.adminToken,
    repoId,
    "/",
    "long.txt",
    Array.from({ length: 400 }, (_, i) => `line ${i}`).join("\n"),
  );
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

const dblclickRow = (page: import("@playwright/test").Page, name: string) =>
  page
    .locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] > div:first-child`)
    .dblclick();

test("image preview shows the image", async ({ page }) => {
  await dblclickRow(page, "photo.png");
  await expect(page.locator("#quick-preview-overlay")).toBeVisible();
  await expect(page.locator(".js-qp-img")).toBeVisible();
});

test("text preview shows the file content", async ({ page }) => {
  await dblclickRow(page, "alpha.txt");
  await expect(page.locator("#quick-preview-overlay")).toBeVisible();
  await expect(page.locator(".js-qp-text")).toContainText("content of alpha.txt");
});

test("audio preview opens the audio player", async ({ page }) => {
  await dblclickRow(page, "song.wav");
  await expect(page.locator(".js-qp-audio")).toBeVisible();
});

test("video preview opens the video player", async ({ page }) => {
  await dblclickRow(page, "plays.mp4");
  await expect(page.locator(".js-qp-video")).toBeVisible();
  // A decodable clip must not be reported as unplayable.
  await expect(page.locator(".js-qp-unsupported")).toBeHidden();
  await expect
    .poll(() => page.locator(".js-qp-video").evaluate((v) => (v as HTMLVideoElement).videoWidth))
    .toBeGreaterThan(0);
});

// A media element that cannot decode its source fires `error` and otherwise
// just sits there, so the dialog used to be an empty black box with no
// explanation. Which files decode is the browser's codec list, not the file
// extension: an HEVC/H.265 clip plays nowhere on a stock Windows Chrome/Edge
// even though the phone writes it into the same .mp4/.MOV container as H.264.
test("an unplayable video says so instead of showing a black box", async ({ page }) => {
  await dblclickRow(page, "clip.mp4");
  const message = page.locator(".js-qp-unsupported");
  await expect(message).toBeVisible();
  await expect(message).toContainText("H.265/HEVC");
  await expect(page.locator(".js-qp-video")).toBeHidden();
});

// The dialog used to be content-sized, so it opened at the media element's
// default 300x150 box and jumped to the clip's real size once its metadata
// arrived. The stage now reserves that space up front.
test("the video dialog is settled before the clip loads", async ({ page }) => {
  await page.evaluate(() => {
    (window as any).__qp = [];
    const video = document.querySelector(".js-qp-video") as HTMLVideoElement;
    const snap = (tag: string) => {
      const card = document.querySelector("#quick-preview-overlay > div") as HTMLElement;
      (window as any).__qp.push({ tag, h: Math.round(card.getBoundingClientRect().height) });
    };
    new MutationObserver(() => {
      if (!video.classList.contains("hidden")) {
        // Recorded the moment the player is revealed, before its metadata can
        // have arrived, then again once it has.
        snap("revealed");
        video.addEventListener("loadedmetadata", () => snap("metadata"), { once: true });
      }
    }).observe(video, { attributes: true, attributeFilter: ["class"] });
  });

  await dblclickRow(page, "plays.mp4");
  await page.waitForFunction(() => (window as any).__qp.some((s: any) => s.tag === "metadata"));

  const shots = await page.evaluate(() => (window as any).__qp as { tag: string; h: number }[]);
  const revealed = shots.find((s) => s.tag === "revealed")!;
  expect(shots[shots.length - 1].h).toBe(revealed.h);
  // …and it is already a preview-sized dialog, not a 150px-tall strip.
  expect(revealed.h).toBeGreaterThan((page.viewportSize()?.height || 0) / 2);
});

test("unsupported files do not open the quick preview", async ({ page }) => {
  await dblclickRow(page, "note.xyz");
  await expect(page.locator("#quick-preview-overlay")).toBeHidden();
});

test("full-page text preview renders the file content", async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files/alpha.txt`);
  await expect(page.locator("pre code")).toContainText("content of alpha.txt");
});

// ─── Full-page previews: the other half of the file manager ─────────────────
// Clicking a file *name* (as opposed to double-clicking the row, which opens the
// quick-preview modal above) opens the full-page preview. It renders in the file
// manager's own column — `.content` -> `.topline` -> viewer, the same frame as
// `files/browser.html` — so these pin that frame: the topline carries the trail
// and the actions, the viewer is the column's remainder, and it is the only
// scroller on the page.

/** Open the full-page preview of `name` through the file list, as a user does. */
async function openFullPreview(page: import("@playwright/test").Page, name: string) {
  await page.goto(`/libraries/${repoId}/files/`);
  await page.waitForSelector(".js-entry-row");
  await page.locator(`.js-file-list-view .js-entry-row[data-name="${name}"] a.base`).click();
  await page.waitForSelector(".topline .nf-preview-meta");
}

const boxOf = async (page: import("@playwright/test").Page, selector: string) =>
  (await page.locator(selector).boundingBox())!;

test("the preview renders in the file manager's own column", async ({ page }) => {
  // The file list is the reference frame: the preview's topline has to sit at the
  // same x and span the same width, or the content edge jumps when a name is
  // clicked.
  await page.goto(`/libraries/${repoId}/files/`);
  await page.waitForSelector(".js-entry-row");
  const listTopline = await boxOf(page, "main > .topline");

  await openFullPreview(page, "photo.png");

  // The shared breadcrumb, ending at the file itself (non-link current page), and
  // the whole name on hover now that no title row carries it.
  await expect(page.locator(".crumbs")).toHaveCount(1);
  await expect(page.locator(".crumbs .cur")).toHaveText("photo.png");
  await expect(page.locator(".crumbs .cur")).toHaveAttribute("title", "photo.png");
  // …and neither the legacy markup nor the old card shell survives.
  await expect(page.locator(".nf-preview")).toHaveCount(0);
  await expect(page.locator(".card")).toHaveCount(0);
  await expect(page.locator("nav.flex.mb-4")).toHaveCount(0);

  await expect(page.locator("main")).toHaveCSS("padding-left", "0px");
  const previewTopline = await boxOf(page, "main > .topline");
  expect(previewTopline.x).toBeCloseTo(listTopline.x, 0);
  expect(previewTopline.width).toBeCloseTo(listTopline.width, 0);

  // Size and actions: a size, and real buttons the same height as the toolbar's,
  // all in the topline band.
  await expect(page.locator(".topline .nf-preview-meta")).toHaveText(/\d/);
  const download = page.locator(".topline .btn", { hasText: "Download" });
  await expect(download).toHaveAttribute("href", /\?dl=1$/);
  await expect(download).toHaveCSS("height", "28px");
  await expect(page.locator(".topline .btn", { hasText: "Back to folder" })).toHaveAttribute(
    "href",
    `/libraries/${repoId}/files/`,
  );
});

// The spacing bug the redesign fixes: the crumbs sat on the padded page and the
// viewer in a card below them, so the two bands had no relation to each other.
test("the viewer sits flush under the topline", async ({ page }) => {
  for (const [name, viewer] of [
    ["photo.png", ".nf-preview-stage"],
    ["alpha.txt", ".nf-preview-text"],
  ]) {
    await openFullPreview(page, name);
    const topline = await boxOf(page, "main > .topline");
    const box = await boxOf(page, viewer);
    expect(box.y).toBeCloseTo(topline.y + topline.height, 0);
  }
});

test("the image preview loads from the content endpoint", async ({ page }) => {
  await openFullPreview(page, "photo.png");

  const img = page.locator(".nf-preview-image");
  await expect(img).toHaveAttribute("data-preview-image", "");
  await expect(img).toHaveAttribute("src", new RegExp(`^/repos/${repoId}/files/photo\\.png`));
  await expect(img).toHaveAttribute("data-download-url", /\?dl=1$/);
  await expect.poll(async () => img.evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(1);

  // The stage is the raised media surface the tiles use, and it is exactly the
  // column's remainder — nothing under it, no page-level scroll.
  const stage = page.locator(".nf-preview-stage");
  await expect(stage).toHaveCount(1);
  await expect(stage).toHaveCSS("background-color", "rgb(240, 240, 240)");
  const stageBox = await boxOf(page, ".nf-preview-stage");
  const mainBox = await boxOf(page, "main");
  expect(stageBox.y + stageBox.height).toBeCloseTo(mainBox.y + mainBox.height, 0);
});

// The old rule was a `calc(100dvh - 16rem)` guess at the page chrome, so an image
// bigger than the stage decided the page's height instead of being contained.
test("a picture larger than the stage is contained by it", async ({ page }) => {
  await openFullPreview(page, "big.png");

  const stage = await boxOf(page, ".nf-preview-stage");
  const img = await boxOf(page, ".nf-preview-image");
  expect(img.width).toBeLessThanOrEqual(stage.width + 1);
  expect(img.height).toBeLessThanOrEqual(stage.height + 1);
  // …and it did not collapse to the replaced element's 300x150 default.
  expect(Math.max(img.width, img.height)).toBeGreaterThan(200);
});

test("the media preview uses a native player and a short stage for audio", async ({ page }) => {
  await openFullPreview(page, "clip.mp4");
  const video = page.locator(".nf-preview-video");
  await expect(video).toHaveAttribute("controls", "");
  await expect(video).toHaveAttribute("src", new RegExp(`^/repos/${repoId}/files/clip\\.mp4`));

  await openFullPreview(page, "song.wav");
  await expect(page.locator(".nf-preview-stage.is-audio audio[controls]")).toHaveCount(1);
  await expect(page.locator(".nf-preview-video")).toHaveCount(0);
  // Audio gets the shorter stage: a wide empty band around a control strip
  // reads as a mistake.
  const stage = await page
    .locator(".nf-preview-stage")
    .evaluate((el) => Math.round(el.getBoundingClientRect().height));
  expect(stage).toBeLessThan(200);
});

test("the text preview renders in the column in monospace", async ({ page }) => {
  await openFullPreview(page, "alpha.txt");
  const pre = page.locator(".nf-preview-text");
  await expect(pre).toContainText("content of alpha.txt");
  await expect(pre).toHaveCSS("font-family", /mono/i);
  await expect(page.locator(".nf-preview-stage")).toHaveCount(0);
});

// The column and the code block both scrolled before; the block is the only
// scroller now, so the topline can never be pushed off screen by the text.
test("a long text preview scrolls in the code block, not the column", async ({ page }) => {
  await openFullPreview(page, "long.txt");
  expect(await page.locator("main").evaluate((el) => el.scrollHeight - el.clientHeight)).toBe(0);
  expect(
    await page.locator(".nf-preview-text").evaluate((el) => el.scrollHeight - el.clientHeight),
  ).toBeGreaterThan(0);
});

// The mobile bottom nav is fixed over the column's last 5rem. The viewer keeps
// the same clearance `.nf-scroll` gives the list, so the final line of a long
// file can be scrolled clear of the nav instead of hiding under it.
test("the viewer keeps clear of the mobile bottom nav", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openFullPreview(page, "long.txt");

  await page.locator(".nf-preview-text").evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  const lastLine = await page.evaluate(() => {
    const code = document.querySelector(".nf-preview-text code")!;
    const range = document.createRange();
    range.selectNodeContents(code);
    const rects = range.getClientRects();
    return rects[rects.length - 1].bottom;
  });
  const nav = await boxOf(page, "body > nav");
  expect(lastLine).toBeLessThanOrEqual(nav.y);
});

test("the viewer follows the theme", async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files/`);
  await page.evaluate(() => localStorage.removeItem("darkMode"));
  await openFullPreview(page, "photo.png");
  const light = await page
    .locator(".nf-preview-stage")
    .evaluate((el) => getComputedStyle(el).backgroundColor);

  await page.evaluate(() => localStorage.setItem("darkMode", "true"));
  await openFullPreview(page, "photo.png");
  const dark = await page
    .locator(".nf-preview-stage")
    .evaluate((el) => getComputedStyle(el).backgroundColor);

  expect(light).toBe("rgb(240, 240, 240)");
  expect(dark).toBe("rgb(35, 35, 35)");
});

