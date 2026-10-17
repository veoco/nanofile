import { test, expect } from "@playwright/test";
import { bannerGap, tabsGap } from "../helpers/layout";

// Asserts the default English UI (no admin language override), which renders the
// Periodic/Service labels and job names from the en locale.

const REGISTRY = "/sysadmin/tasks/registered/";

const taskRow = (page: import("@playwright/test").Page, name: string) =>
  page.locator(`main [data-task="${name}"]`);

// The two views of the task system are sibling pages: a bookmark to either has to mean
// something, so the tab bar marks the current one.
test("the two task pages are a tab apart", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  const tabs = page.locator("main .nf-tabs a");
  await expect(tabs).toHaveCount(2);
  await expect(page.locator('main .nf-tabs a[aria-current="page"]')).toHaveText(
    "Task list",
  );

  await tabs.filter({ hasText: "Registered tasks" }).click();
  await expect(page).toHaveURL(/\/sysadmin\/tasks\/registered\/$/);
  await expect(page.locator('main .nf-tabs a[aria-current="page"]')).toHaveText(
    "Registered tasks",
  );
});

// The registry is grouped by how a job comes to run, so the heading says what a
// badge used to repeat on every row.
test("the registry groups jobs by how they run", async ({ page }) => {
  await page.goto(REGISTRY);
  const periodic = page.locator('main [data-task-group="periodic"]');
  const onDemand = page.locator('main [data-task-group="on_demand"]');
  await expect(periodic.locator(".nf-sec h2")).toHaveText("Periodic");
  await expect(onDemand.locator(".nf-sec h2")).toHaveText("On demand");

// A job a request submits offers no trigger button — a bare button cannot supply its
// parameters — and the row says when it runs instead.
  await expect(taskRow(page, "copy")).toBeVisible();
  await expect(taskRow(page, "copy").locator("form.trigger-form")).toHaveCount(0);
  await expect(taskRow(page, "expired-data-cleanup").locator("form.trigger-form")).toHaveCount(1);

// A long-lived service is its own group and has no counters: it never finishes.
  const services = page.locator('main [data-task-group="service"] [data-task-kind="service"]');
  if ((await services.count()) > 0) {
    await expect(services.first().locator("[data-counter]")).toHaveCount(0);
    await expect(services.first().locator("form.trigger-form")).toHaveCount(0);
  }
});

// The registry wraps each group in a `<section>`, so the heading's own hairline is the
// boundary: the row that closes a group must not draw one, or the two read as one thick line.
test("a group heading is the only line between two groups", async ({ page }) => {
  await page.goto(REGISTRY);
  const periodic = page.locator('main [data-task-group="periodic"]');
  const onDemand = page.locator('main [data-task-group="on_demand"]');

  await expect(periodic.locator(".nf-xrow").last()).toHaveCSS("border-bottom-width", "0px");
  await expect(onDemand.locator(".nf-sec")).toHaveCSS("border-top-width", "1px");
  await expect(onDemand.locator(".nf-sec")).toHaveCSS("border-bottom-width", "1px");
  await expect(periodic.locator(".nf-sec")).toHaveCSS("border-top-width", "0px");
});

// Zero and "no such number" must not look the same.
test("a job that has never run shows no counters", async ({ page }) => {
  await page.goto(REGISTRY);
  const never = page
    .locator('main [data-task-kind="job"]')
    .filter({ hasText: "Has not run yet" });
  test.skip((await never.count()) === 0, "every job has already run on this server");
  await expect(never.first()).toContainText("Has not run yet");
  await expect(never.first().locator("[data-counter]")).toHaveCount(0);
});

// The registry columns let jobs be compared down the list: last run and duration in the same
// place, whether a time, "has not run yet", or a dash for a service.
test("the registry columns line up down the list", async ({ page }) => {
  await page.goto(REGISTRY);
  const columns = await page
    .locator('main [data-fact="last_run"]')
    .evaluateAll((els) =>
      els.map((el) => {
        const box = el.getBoundingClientRect();
        return { right: Math.round(box.right), width: Math.round(box.width) };
      }),
    );

  expect(columns.length).toBeGreaterThanOrEqual(2);
  for (const column of columns) {
    expect(column.width).toBe(112);
    expect(Math.abs(column.right - columns[0].right)).toBeLessThanOrEqual(1);
  }
});

// The lifetime counters are reference data, not something the list is scanned for, so they are
// one disclosure away — labelled when they open.
test("a job's counters are labelled behind its disclosure", async ({ page }) => {
  await page.goto(REGISTRY);
  await taskRow(page, "expired-data-cleanup")
    .locator('form.trigger-form button[type="submit"]')
    .click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/\/sysadmin\/tasks\/registered\/\?action=triggered$/);

  // A run has to be recorded before the job reports a counter.
  await expect
    .poll(async () => {
      await page.goto(REGISTRY);
      return taskRow(page, "expired-data-cleanup").locator("[data-counter]").count();
    })
    .toBeGreaterThan(0);

  const detail = taskRow(page, "expired-data-cleanup").locator("details.nf-xrow-more");
  await expect(detail.locator("[data-counter]").first()).toBeHidden();
  await detail.locator("summary").click();
  await expect(detail).toContainText("Totals");
  await expect(detail.locator("[data-counter]").first()).toContainText("Runs");
});

// The policies are a disclosure rather than another line on every row; a service has none.
test("a job's scheduling policy is one disclosure away", async ({ page }) => {
  await page.goto(REGISTRY);
  const row = taskRow(page, "expired-data-cleanup");
  const detail = row.locator("details.nf-xrow-more");
  await expect(detail.locator("summary")).toContainText("Stats and policy");
  await expect(detail.locator(".nf-kv").first()).toBeHidden();

  await detail.locator("summary").click();
  for (const label of [
    "Priority",
    "Contends for",
    "Concurrent runs",
    "Timeout",
    "Retry",
    "Waits for a quiet server",
    "Interruptible",
    "Can be stopped",
    "After a crash",
  ]) {
    await expect(detail).toContainText(label);
  }

  const services = page.locator('main [data-task-kind="service"]');
  if ((await services.count()) > 0) {
    await expect(services.first().locator("details")).toHaveCount(0);
  }
});

// The two pages hold different things, so neither should be showing the other's
// rows.
test("the run list is not the registry", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  await expect(page.locator("main .nf-sec h2").first()).toHaveText("Running now");
  await expect(page.locator("main [data-task]")).toHaveCount(0);
});

// A job whose subsystem is off is not registered at all, which is right and also invisible:
// without this panel it reads as a job the page forgot.
test("a declared job this server does not run says why", async ({ page }) => {
  await page.goto(REGISTRY);
  await expect(
    page.locator("main .nf-sec h2", { hasText: "Not registered on this server" }),
  ).toBeVisible();
  const gc = page.locator('main [data-task-skipped="gc"]');
  await expect(gc).toContainText("Garbage collection");
  await expect(gc.locator(".nf-prow-hi")).not.toBeEmpty();
  await expect(page.locator("main [data-task-skipped]").first()).not.toContainText("admin.");
});

// The journal is durable, so a finished run shows up named and labelled rather than as the
// slug and wire phase the database holds.
test("a finished run is named and labelled on the run list", async ({ page }) => {
  await page.goto(REGISTRY);
  await taskRow(page, "expired-data-cleanup")
    .locator('form.trigger-form button[type="submit"]')
    .click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/\/sysadmin\/tasks\/registered\/\?action=triggered$/);

// The run is queued rather than executed in the request, so the row appears once it finishes.
  await expect
    .poll(async () => {
      await page.goto("/sysadmin/tasks/");
      return page
        .locator('main .nf-xrow[data-run]')
        .filter({ hasText: "Expired data cleanup" })
        .count();
    })
    .not.toBe(0);

  const run = page
    .locator('main .nf-xrow[data-run]')
    .filter({ hasText: "Expired data cleanup" })
    .first();
  await expect(run.locator(".badge")).toHaveText("Succeeded");
  await expect(run).not.toContainText("expired-data-cleanup");
// The row's second line is what the run did, not who submitted it: every scheduled run is
// submitted by the server.
  await expect(run.locator("summary.nf-prow .nf-prow-sub")).toContainText("expired");
  await expect(run.locator(".nf-xrow-more [data-fact='owner']")).toContainText("The server");

// The row is its own disclosure: the id and the job's own report are one click away, and the
// report is what the job said, never the name the row already carries.
  await expect(run.locator(".nf-xrow-more .nf-kv").first()).toBeHidden();
  await run.locator("summary.nf-prow").click();
  await expect(run.locator(".nf-xrow-more")).toContainText("Run id");
  await expect(run.locator(".nf-xrow-more")).toContainText("Summary");
  const summary = run.locator(".nf-xrow-more .nf-kv", { hasText: "Summary" });
  await expect(summary.locator(".v")).not.toHaveText("Expired data cleanup");
  await expect(summary.locator(".v")).toContainText("expired");
});

// The run columns are fixed-width and right-aligned, so the finish times can be
// compared down the list instead of read one row at a time.
test("the run columns line up down the list", async ({ page }) => {
  // Two recorded runs, so there is something to line up across. The first has
  // to reach the journal before the second is submitted, or the job's own
  // concurrency cap turns the duplicate away.
  for (let i = 0; i < 2; i++) {
    await page.goto(REGISTRY);
    await taskRow(page, "expired-data-cleanup")
      .locator('form.trigger-form button[type="submit"]')
      .click();
    await page.locator(".js-confirm-ok").click();
    await page.waitForURL(/\/sysadmin\/tasks\/registered\/\?action=triggered$/);
    await expect
      .poll(async () => {
        await page.goto("/sysadmin/tasks/");
        return page.locator('main [data-fact="finished"]').count();
      })
      .toBeGreaterThanOrEqual(i + 1);
  }

  const columns = await page
    .locator('main [data-fact="finished"]')
    .evaluateAll((els) =>
      els.map((el) => {
        const box = el.getBoundingClientRect();
        return { right: Math.round(box.right), width: Math.round(box.width) };
      }),
    );

  expect(columns.length).toBeGreaterThanOrEqual(2);
  for (const column of columns) {
    expect(column.width).toBe(112);
    expect(Math.abs(column.right - columns[0].right)).toBeLessThanOrEqual(1);
  }
});

// The list is filtered by job and by verdict, and each filter keeps the other.
test("the run list is filtered by job and by verdict", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  await expect(page.locator('main [data-panel="recent"]')).toBeVisible();

  const jobChips = page.locator('main [data-panel="recent"] a[data-filter]');
  await expect(jobChips.filter({ hasText: "Expired data cleanup" })).toHaveCount(1);

  // Each group's "everything" names its own dimension: two chips reading "All"
  // side by side left the reader to tell them apart by position alone.
  const allChips = page.locator('main [data-panel="recent"] a[data-filter=""]');
  await expect(allChips).toHaveCount(2);
  await expect(allChips.first()).toHaveText("All jobs");
  await expect(allChips.last()).toHaveText("All results");

// Narrow to a job, then to its failures, and check the second filter kept the first.
  await jobChips.filter({ hasText: "Expired data cleanup" }).click();
  await expect(page).toHaveURL(/\?kind=expired-data-cleanup$/);
  await expect(allChips.last()).toHaveAttribute(
    "href",
    "/sysadmin/tasks/?kind=expired-data-cleanup",
  );

  await page.locator('main [data-panel="recent"] a[data-filter="failed"]').click();
  await expect(page).toHaveURL(/kind=expired-data-cleanup(?:&#38;|&)outcome=failed$/);

// Nothing failed, so the list says the filter is why it is empty rather than claiming no run.
  await expect(page.locator('main [data-panel="recent"]')).toContainText(
    "No recorded run matches this filter",
  );
  await expect(page.locator('main [data-panel="recent"] [data-run]')).toHaveCount(0);

  // Clearing one dimension leaves the other alone: the job row's "All jobs" keeps
  // the verdict, and the verdict group's "All results" then clears it too.
  await allChips.first().click();
  await expect(page).toHaveURL(/\/sysadmin\/tasks\/\?outcome=failed$/);
  await allChips.last().click();
  await expect(page).toHaveURL(/\/sysadmin\/tasks\/$/);
  await expect(page.locator('main [data-panel="recent"] [data-run]').first()).toBeVisible();
});

// The tab bar's bottom margin must keep the load card from reading as part of the tab bar.
test("the first block sits clear of the tab bar", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  expect(await tabsGap(page)).toBeGreaterThanOrEqual(16);
});

// The load measurements explain a run that is waiting, so they come first.
test("the server load sits above the run lists", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  const at = async (panel: string) =>
    (await page.locator(`main [data-panel="${panel}"]`).boundingBox())?.y;

  const load = await at("load");
  const active = await at("active");
  const recent = await at("recent");
  expect(load).toBeDefined();
  expect(active).toBeDefined();
  expect(recent).toBeDefined();
  expect(load!).toBeLessThan(active!);
  expect(load!).toBeLessThan(recent!);
});

test("trigger a periodic task manually", async ({ page }) => {
  await page.goto(REGISTRY);
  const row = taskRow(page, "expired-data-cleanup");
  await expect(row).toBeVisible();
  await expect(row.locator("form.trigger-form")).toBeVisible();

// The trigger's visible effect is one disclosure away: the row's text can look unchanged.
  const runs = async () => {
    await page.goto(REGISTRY);
    const counter = taskRow(page, "expired-data-cleanup")
      .locator("details.nf-xrow-more")
      .locator("[data-counter]")
      .first();
    if ((await counter.count()) === 0) return 0;
    await taskRow(page, "expired-data-cleanup").locator("details.nf-xrow-more summary").click();
    return Number((await counter.innerText()).replace(/\D+/g, ""));
  };
  const before = await runs();

  await row.locator('form.trigger-form button[type="submit"]').click();
  await page.locator(".js-confirm-ok").click();
// The button lives on the registry, so the confirmation lands here rather than on the run list.
  await page.waitForURL(/\/sysadmin\/tasks\/registered\/\?action=triggered$/);
  await expect(page.locator("main .nf-banner.is-ok")).toContainText("Task triggered");
// The banner is its own block: the description has no bottom margin, so the banner keeps the gap.
  expect(await bannerGap(page)).toBeGreaterThanOrEqual(16);

  await expect.poll(runs).toBeGreaterThan(before);
});

// A browser form must get a page back whatever happens: an unknown task slug
// re-renders the registry with the reason, not the API's JSON `error_msg` body.
test("an unknown task reports instead of answering with JSON", async ({ page }) => {
  await page.goto(REGISTRY);
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
