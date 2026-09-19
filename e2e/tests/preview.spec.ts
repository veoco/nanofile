import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";

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
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", "note.xyz", "unrecognized\n");
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
  await dblclickRow(page, "clip.mp4");
  await expect(page.locator(".js-qp-video")).toBeVisible();
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
// quick-preview modal above) opens the full-page preview. It was the last page
// in the file-manager flow still on the legacy layout — old breadcrumb, `.card`,
// `gray-*` panels — so these pin the redesigned chrome.

/** Open the full-page preview of `name` through the file list, as a user does. */
async function openFullPreview(page: import("@playwright/test").Page, name: string) {
  await page.goto(`/libraries/${repoId}/files/`);
  await page.waitForSelector(".js-entry-row");
  await page.locator(`.js-file-list-view .js-entry-row[data-name="${name}"] a.base`).click();
  await page.waitForSelector(".nf-preview");
}

test("a file name opens the preview panel, not the legacy card", async ({ page }) => {
  await openFullPreview(page, "photo.png");

  // The shared breadcrumb, ending at the file itself (non-link current page).
  await expect(page.locator(".crumbs")).toHaveCount(1);
  await expect(page.locator(".crumbs .cur")).toHaveText("photo.png");
  // …and none of the legacy markup survives.
  await expect(page.locator(".card")).toHaveCount(0);
  await expect(page.locator("nav.flex.mb-4")).toHaveCount(0);

  // Header: the name, a size, and real buttons the same height as the toolbar's.
  await expect(page.locator(".nf-preview-title")).toHaveText("photo.png");
  await expect(page.locator(".nf-preview-meta")).toHaveText(/\d/);
  const download = page.locator(".nf-preview-head .btn", { hasText: "Download" });
  await expect(download).toHaveAttribute("href", /\?dl=1$/);
  await expect(download).toHaveCSS("height", "28px");
  await expect(
    page.locator(".nf-preview-head .btn", { hasText: "Back to folder" }),
  ).toHaveAttribute("href", `/libraries/${repoId}/files/`);

  // The panel is on the shared surface tokens, not an unstyled default.
  await expect(page.locator(".nf-preview")).toHaveCSS("background-color", "rgb(249, 249, 249)");
});

test("the image preview loads from the content endpoint", async ({ page }) => {
  await openFullPreview(page, "photo.png");

  const img = page.locator(".nf-preview-image");
  await expect(img).toHaveAttribute("data-preview-image", "");
  await expect(img).toHaveAttribute("src", new RegExp(`^/repos/${repoId}/files/photo\\.png`));
  await expect(img).toHaveAttribute("data-download-url", /\?dl=1$/);
  await expect.poll(async () => img.evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(1);
  await expect(page.locator(".nf-preview-stage")).toHaveCount(1);
});

test("the media preview uses a native player and a short stage for audio", async ({ page }) => {
  await openFullPreview(page, "clip.mp4");
  const video = page.locator(".nf-preview-video");
  await expect(video).toHaveAttribute("controls", "");
  await expect(video).toHaveAttribute("src", new RegExp(`^/repos/${repoId}/files/clip\\.mp4`));

  await openFullPreview(page, "song.wav");
  await expect(page.locator(".nf-preview-stage audio[controls]")).toHaveCount(1);
  await expect(page.locator(".nf-preview-video")).toHaveCount(0);
  // Audio gets the shorter stage: a wide empty band around a control strip
  // reads as a mistake.
  const stage = await page
    .locator(".nf-preview-stage")
    .evaluate((el) => Math.round(el.getBoundingClientRect().height));
  expect(stage).toBeLessThan(200);
});

test("the text preview renders inside the panel in monospace", async ({ page }) => {
  await openFullPreview(page, "alpha.txt");
  const pre = page.locator(".nf-preview-text");
  await expect(pre).toContainText("content of alpha.txt");
  await expect(pre).toHaveCSS("font-family", /mono/i);
  await expect(page.locator(".nf-preview-stage")).toHaveCount(0);
});

test("the preview panel follows the theme", async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files/`);
  await page.evaluate(() => localStorage.removeItem("darkMode"));
  await openFullPreview(page, "photo.png");
  const light = await page
    .locator(".nf-preview")
    .evaluate((el) => getComputedStyle(el).backgroundColor);

  await page.evaluate(() => localStorage.setItem("darkMode", "true"));
  await openFullPreview(page, "photo.png");
  const dark = await page
    .locator(".nf-preview")
    .evaluate((el) => getComputedStyle(el).backgroundColor);

  expect(light).toBe("rgb(249, 249, 249)");
  expect(dark).toBe("rgb(25, 25, 25)");
});

