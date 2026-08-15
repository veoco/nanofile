import { test, expect } from "@playwright/test";

// Asserts the default English UI (default_language=en; the admin has no
// language override), which renders the Periodic/Continuous/Never labels.

const taskRow = (page: import("@playwright/test").Page, name: string) =>
  page.locator("main table tbody tr", { hasText: name });

test("tasks page lists scheduled periodic and continuous tasks", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  await expect(taskRow(page, "share link cleanup")).toBeVisible();
  await expect(page.getByText("Periodic").first()).toBeVisible();
  await expect(page.getByText("Continuous").first()).toBeVisible();
});

test("trigger a periodic task manually", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  const row = taskRow(page, "share link cleanup");
  await expect(row).toBeVisible();
  // Periodic tasks expose a trigger button (continuous ones don't).
  await expect(row.locator("form.trigger-form")).toBeVisible();
  // Column 4 is "Last run" (name/type/interval/last_run/...).
  const lastRunBefore = (await row.locator("td").nth(3).innerText()).trim();

  page.once("dialog", (dialog) => dialog.accept());
  await row.locator('form.trigger-form button[type="submit"]').click();
  await page.waitForURL(/\/sysadmin\/tasks\/$/);

  // A manual run stamps a new last-run timestamp.
  await expect
    .poll(async () =>
      (await taskRow(page, "share link cleanup").locator("td").nth(3).innerText()).trim(),
    )
    .not.toBe(lastRunBefore);
});
