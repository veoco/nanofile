import { test, expect } from "@playwright/test";
import { bannerGap, subtitleGap } from "../helpers/layout";

// Asserts the default English UI (default_language=en; the admin has no
// language override), which renders the Periodic/Continuous/Never labels.

const taskRow = (page: import("@playwright/test").Page, name: string) =>
  page.locator(`main .nf-prow[data-name="${name}"]`);

/** The row's last-run cell, filled from `data-ts` by core/local-time.js. */
const lastRun = (page: import("@playwright/test").Page, name: string) =>
  taskRow(page, name).locator("[data-last-run]");

test("tasks page lists scheduled periodic and continuous tasks", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  await expect(taskRow(page, "share link cleanup")).toBeVisible();
  await expect(page.getByText("Periodic").first()).toBeVisible();
  await expect(page.getByText("Continuous").first()).toBeVisible();
});

// The subtitle has no bottom margin of its own, so the task list must keep its
// own top margin or the two touch.
test("the description sits clear of the task table", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  expect(await subtitleGap(page)).toBeGreaterThanOrEqual(16);
});

test("trigger a periodic task manually", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  const row = taskRow(page, "share link cleanup");
  await expect(row).toBeVisible();
  // Periodic tasks expose a trigger button (continuous ones don't).
  await expect(row.locator("form.trigger-form")).toBeVisible();
  const lastRunBefore = (await lastRun(page, "share link cleanup").innerText()).trim();

  await row.locator('form.trigger-form button[type="submit"]').click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/\/sysadmin\/tasks\/\?action=triggered$/);
  await expect(page.locator("main .nf-banner.is-ok")).toContainText("Task triggered");
  // The banner is a block in its own right: the description has no bottom
  // margin, so the banner has to keep its distance.
  expect(await bannerGap(page)).toBeGreaterThanOrEqual(16);

  // A manual run stamps a new last-run timestamp.
  await expect
    .poll(async () =>
      (await lastRun(page, "share link cleanup").innerText()).trim(),
    )
    .not.toBe(lastRunBefore);
});

// A browser form must get a page back whatever happens: an unknown task name
// re-renders the list with the reason, not the API's JSON `error_msg` body.
test("an unknown task reports instead of answering with JSON", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  const csrf = await page
    .locator('main form.trigger-form input[name="csrf_token"]')
    .first()
    .inputValue();

  const resp = await page.request.post("/sysadmin/tasks/no-such-task/trigger/", {
    form: { csrf_token: csrf },
  });

  expect(resp.status()).toBe(200);
  expect(resp.headers()["content-type"]).toContain("text/html");
  const body = await resp.text();
  expect(body).toContain("nf-banner is-err");
  expect(body).toContain("No task named");
});
