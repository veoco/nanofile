import { test, expect } from "@playwright/test";
import { readState, seedRepo, createRepo, uploadFile } from "../helpers/api";

let state: ReturnType<typeof readState>;
let nameRepoId: string;
let sizeRepoId: string;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

test.beforeAll(async () => {
  state = readState();
  nameRepoId = await seedRepo(state.baseURL, state.adminToken, "sort-repo");
  // A second repo with clearly distinct sizes and (via sleeps) mtimes so the
  // size/mtime sorts have a deterministic order.
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
  // Directories always sort first, so assert on file rows only.
  const fileRows = page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-type="file"]');
  await expect(fileRows.nth(0)).toHaveAttribute("data-name", "alpha.txt");
  await expect(fileRows.nth(3)).toHaveAttribute("data-name", "delta.txt");
  await page.locator('.js-sort-btn[data-sort="name"]').click();
  await expect(fileRows.nth(0)).toHaveAttribute("data-name", "delta.txt");
  await expect(fileRows.nth(3)).toHaveAttribute("data-name", "alpha.txt");
});

test("sort by size ascending then descending", async ({ page }) => {
  await page.goto(`/libraries/${sizeRepoId}/files`);
  await page.waitForSelector(".js-entry-row");
  const rows = page.locator('.js-file-list-view:not(.hidden) .js-entry-row');
  const sizeBtn = page.locator('.js-sort-btn[data-sort="size"]');
  // First click: ascending (small → large).
  await sizeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "small.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "large.txt");
  // Second click: descending (large → small).
  await sizeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "large.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "small.txt");
});

test("sort by mtime ascending then descending", async ({ page }) => {
  await page.goto(`/libraries/${sizeRepoId}/files`);
  await page.waitForSelector(".js-entry-row");
  const rows = page.locator('.js-file-list-view:not(.hidden) .js-entry-row');
  const mtimeBtn = page.locator('.js-sort-btn[data-sort="mtime"]');
  // Upload order was small → medium → large, so mtime asc keeps that order.
  await mtimeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "small.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "large.txt");
  await mtimeBtn.click();
  await expect(rows.nth(0)).toHaveAttribute("data-name", "large.txt");
  await expect(rows.last()).toHaveAttribute("data-name", "small.txt");
});
