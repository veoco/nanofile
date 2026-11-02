import { test, expect } from "@playwright/test";
import { readState, createRepo, uploadFiles } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

// Four pages: the old client dropped page 1 as soon as the fourth one arrived.
// The extra entry makes the last page partial, which is also the normal case.
const PAGE = 200;
const FILES = 601;

test.beforeAll(async () => {
  state = readState();
  repoId = await createRepo(state.baseURL, state.adminToken, `list-paging-${Date.now()}`);
  const names = Array.from({ length: FILES }, (_, i) => `p-${String(i).padStart(4, "0")}.txt`);
  await uploadFiles(state.baseURL, state.adminToken, repoId, "/", names, "x");
});

// Loading every page used to recycle the oldest page out of the DOM. Scrolling
// back then stopped at the start of the retained window, in the middle of the
// folder, and earlier files could only be reached by reloading the page.
test("scrolling back after many pages reaches the first entry", async ({ page }) => {
  await page.goto(`/libraries/${repoId}/files/`);
  const rows = page.locator(".js-file-list-view .js-entry-row");
  await expect(rows).toHaveCount(PAGE);

  for (const expected of [400, 600, FILES]) {
    await page.locator("#nf-list-scroll").evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await expect(rows).toHaveCount(expected);
  }

  await expect(page.locator(".js-load-more-bar")).toBeHidden();
  await expect(page.locator(".js-file-list-view")).toHaveAttribute("data-has-more", "false");

  await page.locator("#nf-list-scroll").evaluate((el) => {
    el.scrollTop = 0;
  });
  await expect(rows.first()).toHaveAttribute("data-name", "p-0000.txt");
  await expect(rows).toHaveCount(FILES);
});
