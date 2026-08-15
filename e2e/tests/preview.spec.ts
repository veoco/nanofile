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
