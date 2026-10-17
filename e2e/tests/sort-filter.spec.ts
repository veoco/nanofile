import { test, expect } from "@playwright/test";
import { readState, seedRepo, createRepo, uploadFile } from "../helpers/api";
import { localStamp } from "../helpers/time";

let state: ReturnType<typeof readState>;
let nameRepoId: string;
let sizeRepoId: string;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

test.beforeAll(async () => {
  state = readState();
  nameRepoId = await seedRepo(state.baseURL, state.adminToken, "sort-repo");
// A second repo with distinct sizes and (via sleeps) mtimes, so size/mtime sorts are deterministic.
  sizeRepoId = await createRepo(state.baseURL, state.adminToken, "sort-size-repo");
  await uploadFile(state.baseURL, state.adminToken, sizeRepoId, "/", "small.txt", "x");
  await sleep(1100);
  await uploadFile(state.baseURL, state.adminToken, sizeRepoId, "/", "medium.txt", "y".repeat(20));
  await sleep(1100);
  await uploadFile(state.baseURL, state.adminToken, sizeRepoId, "/", "large.txt", "z".repeat(100));
});

test("sort by name toggles file order", async ({ page }) => {
  await page.goto(`/libraries/${nameRepoId}/files`);
  await page.waitForSelector(".js-entry-row");
  const fileRows = page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-type="file"]');
  await expect(fileRows.nth(0)).toHaveAttribute("data-name", "alpha.txt");
  await expect(fileRows.nth(3)).toHaveAttribute("data-name", "delta.txt");
  await page.locator('.js-sort-btn[data-sort="name"]').click();
  await expect(fileRows.nth(0)).toHaveAttribute("data-name", "delta.txt");
  await expect(fileRows.nth(3)).toHaveAttribute("data-name", "alpha.txt");
// After the sort refresh, the new rows must still be visited by local-time.js: the tooltip
// carries the exact local time (regression: the observer died with the container).
  const refreshed = fileRows.nth(0).locator(".nf-when");
  await expect(refreshed).toHaveAttribute("title", await localStamp(refreshed, "tooltip"));
});

// The Modified column reads like the library list's, both via `data-ts-mode="relative"`.
test("the modified column uses the library list's relative form", async ({ page }) => {
  await page.goto(`/libraries/${sizeRepoId}/files`);
  await page.waitForSelector(".js-entry-row");
  const cell = page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="large.txt"] .nf-when');
  await expect(cell).toHaveText(/Just now|ago$/);
  await expect(cell).not.toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  await expect(cell).toHaveAttribute("title", await localStamp(cell, "tooltip"));
});

// A page left open must not keep claiming "Just now": relative cells re-render on a minute tick.
test("a relative timestamp gets older while the page stays open", async ({ page }) => {
  await page.clock.install();
  await page.goto(`/libraries/${sizeRepoId}/files`);
  await page.waitForSelector(".js-entry-row");
  const cell = page
    .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="large.txt"] .nf-when');
  await expect(cell).toHaveText(/Just now|ago$/);

  await page.clock.fastForward("02:00");
  await expect(cell).toHaveText(/minute(s)? ago$/);
});

test("sort by size ascending then descending", async ({ page }) => {
  await page.goto(`/libraries/${sizeRepoId}/files`);
  await page.waitForSelector(".js-entry-row");
  const rows = page.locator('.js-file-list-view:not(.hidden) .js-entry-row');
  const sizeBtn = page.locator('.js-sort-btn[data-sort="size"]');
  await sizeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "small.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "large.txt");
  await sizeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "large.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "small.txt");
});

test("sort by mtime ascending then descending", async ({ page }) => {
  await page.goto(`/libraries/${sizeRepoId}/files`);
  await page.waitForSelector(".js-entry-row");
  const rows = page.locator('.js-file-list-view:not(.hidden) .js-entry-row');
  const mtimeBtn = page.locator('.js-sort-btn[data-sort="mtime"]');
  await mtimeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "small.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "large.txt");
  await mtimeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "large.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "small.txt");
});
