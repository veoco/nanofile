import { test, expect } from "@playwright/test";
import { readState, seedRepo, uploadFile } from "../helpers/api";
import { clickRowAction } from "../helpers/details";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

async function openFreshRepo(page: import("@playwright/test").Page): Promise<string> {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  return repoId;
}

const activityRow = (page: import("@playwright/test").Page, name: string) =>
  page.locator("main .nf-prow").filter({ hasText: name }).first();

test("uploading a file records an activity", async ({ page }) => {
  const repoId = await openFreshRepo(page);
  const name = `uploaded-${Date.now()}.txt`;
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", name, "hello\n");
  await page.goto("/activities/");
  await expect(activityRow(page, name)).toBeVisible();
});

test("renaming a file records a rename activity", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  const oldName = `old-${Date.now()}.txt`;
  const newName = `new-${Date.now()}.txt`;
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", oldName, "hello\n");
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await clickRowAction(page, oldName, ".js-rename-btn");
  await page.locator("#rename-input").fill(newName);
  await page.locator('#rename-dialog-form button[type="submit"]').click();
  await expect(
    page.locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${newName}"]`),
  ).toBeVisible({ timeout: 15_000 });

  await page.goto("/activities/");
  // The rename activity row shows both the old and new names.
  const row = activityRow(page, newName);
  await expect(row).toBeVisible();
  await expect(row).toContainText(oldName);
});

test("deleting a file records a delete activity", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  const name = `deleted-${Date.now()}.txt`;
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", name, "hello\n");
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
  await clickRowAction(page, name, ".js-delete-btn");
  await page.locator(".js-confirm-ok").click();
  await expect(
    page.locator(`.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"]`),
  ).toHaveCount(0);

  await page.goto("/activities/");
  await expect(activityRow(page, name)).toBeVisible();
});

// The day headers are cut in the browser, on the reader's local calendar day:
// the server ships the raw timestamp and core/local-time.js inserts the group
// headers, so a header can never disagree with the stamp on the row below it.
test("activity rows are grouped on the reader's local calendar day", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", `grouped-${Date.now()}.txt`, "x");
  await page.goto("/activities/");
  await page.waitForSelector("main .nf-prow[data-ts-day]");

  const groups = await page.evaluate(() => {
    return Array.from(document.querySelectorAll("main .nf-prow[data-ts-day]")).map((row) => {
      const date = new Date(parseInt(row.dataset.tsDay, 10) * 1000);
      const key =
        date.getFullYear() +
        "-" +
        ("0" + (date.getMonth() + 1)).slice(-2) +
        "-" +
        ("0" + date.getDate()).slice(-2);
      const prev = row.previousElementSibling;
      const label = prev && prev.classList.contains("nf-sec") ? prev.textContent.trim() : "";
      return { key, label, title: row.querySelector("[data-ts]").title };
    });
  });

  expect(groups.length).toBeGreaterThan(0);

  // A day header belongs above the first row of each local calendar day and
  // nowhere else: one header per distinct day, never one per row.
  const seen = new Set<string>();
  for (const group of groups) {
    // The row's own stamp renders the day its header claims, in the reader's
    // timezone.
    expect(group.title).toMatch(new RegExp(`^${group.key} \\d{2}:\\d{2}$`));

    if (seen.has(group.key)) {
      expect(group.label).toBe("");
      continue;
    }
    seen.add(group.key);

    expect(group.label).not.toBe("");
    if (group.label !== "Today" && group.label !== "Yesterday") {
      expect(group.label).toBe(group.key);
    }
  }
  expect(seen.size).toBeGreaterThan(0);
  // The regression this guards is a header per *row*, so require a day with
  // more than one row: otherwise the loop above proves nothing.
  expect(groups.length).toBeGreaterThan(seen.size);
});
