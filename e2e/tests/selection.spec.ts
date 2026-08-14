import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

// Rows are rendered once per view (list/grid/gallery); scope to the visible
// list view to avoid strict-mode collisions with the hidden grid/gallery rows.
const row = (name: string) => `.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"]`;
// Click the icon area (first child div) rather than the name <a> link, which
// navigates instead of selecting.
const icon = (name: string) => `${row(name)} > div:first-child`;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "selection-repo");
});

test.beforeEach(async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
});

test("single click selects a row and opens the right panel", async ({ page }) => {
  await page.locator(icon("alpha.txt")).click();
  await expect(page.locator(row("alpha.txt"))).toHaveClass(/selected/);
  await expect(page.locator(".js-rp-name")).toHaveText("alpha.txt");
});

test("clicking another row switches selection", async ({ page }) => {
  await page.locator(icon("alpha.txt")).click();
  await page.locator(icon("bravo.txt")).click();
  await expect(page.locator(row("alpha.txt"))).not.toHaveClass(/selected/);
  await expect(page.locator(row("bravo.txt"))).toHaveClass(/selected/);
});

test("clicking the only selected row deselects it", async ({ page }) => {
  await page.locator(icon("alpha.txt")).click();
  await page.locator(icon("alpha.txt")).click();
  await expect(page.locator(row("alpha.txt"))).not.toHaveClass(/selected/);
});

test("ctrl-click toggles multiple rows", async ({ page }) => {
  await page.locator(icon("alpha.txt")).click();
  await page.locator(icon("bravo.txt")).click({ modifiers: ["Control"] });
  await page.locator(icon("charlie.txt")).click({ modifiers: ["Control"] });
  await expect(page.locator(row("alpha.txt"))).toHaveClass(/selected/);
  await expect(page.locator(row("bravo.txt"))).toHaveClass(/selected/);
  await expect(page.locator(row("charlie.txt"))).toHaveClass(/selected/);
  await expect(page.locator(".js-selection-count")).toHaveText("3");
});

test("shift-click selects a range", async ({ page }) => {
  await page.locator(icon("alpha.txt")).click();
  await page.locator(icon("delta.txt")).click({ modifiers: ["Shift"] });
  await expect(page.locator(row("alpha.txt"))).toHaveClass(/selected/);
  await expect(page.locator(row("bravo.txt"))).toHaveClass(/selected/);
  await expect(page.locator(row("charlie.txt"))).toHaveClass(/selected/);
  await expect(page.locator(row("delta.txt"))).toHaveClass(/selected/);
  await expect(page.locator(".js-selection-count")).toHaveText("4");
});

test("select-all selects every row; deselect-all clears", async ({ page }) => {
  await page.locator("#js-select-all-btn").click();
  await expect(page.locator(".js-selection-count")).toHaveText("5");
  // NOTE: the select-all button's "deselect on second click" branch is broken
  // (it counts hidden grid/gallery rows, so the sizes never match). Deselect
  // via the dedicated .js-deselect-all control instead.
  await page.locator(".js-deselect-all").click();
  await expect(page.locator(".js-entry-row.selected")).toHaveCount(0);
});

test("dblclick opens quick preview for a text file", async ({ page }) => {
  await page.locator(icon("alpha.txt")).dblclick();
  await expect(page.locator("#quick-preview-overlay")).toBeVisible();
  await expect(page.locator(".js-qp-text")).toContainText("content of alpha.txt");
});
