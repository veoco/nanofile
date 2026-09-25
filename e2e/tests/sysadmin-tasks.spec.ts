import { test, expect } from "@playwright/test";
import { bannerGap, tabsGap } from "../helpers/layout";

// Asserts the default English UI (default_language=en; the admin has no
// language override), which renders the Periodic/Service labels and the job
// names from the en locale.

const REGISTRY = "/sysadmin/tasks/registered/";

/** One row of the registry, found by its stable slug rather than its markup. */
const taskRow = (page: import("@playwright/test").Page, name: string) =>
  page.locator(`main [data-task="${name}"]`);

// The two views of the task system are sibling pages, not one page: a bookmark
// to either has to mean something, so the tab bar marks the current one.
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

  // A job a request submits offers no trigger button — a bare button cannot
  // supply its parameters — and the row says when it runs instead.
  await expect(taskRow(page, "copy")).toBeVisible();
  await expect(taskRow(page, "copy").locator("form.trigger-form")).toHaveCount(0);
  await expect(taskRow(page, "share-link-cleanup").locator("form.trigger-form")).toHaveCount(1);

  // A long-lived service is its own group and carries no counters: it never
  // finishes, so it has no run count to show.
  const services = page.locator('main [data-task-group="service"] [data-task-kind="service"]');
  if ((await services.count()) > 0) {
    await expect(services.first().locator("[data-counter]")).toHaveCount(0);
    await expect(services.first().locator("form.trigger-form")).toHaveCount(0);
  }
});

// The registry wraps each group in a `<section>`, so the heading's own hairline
// has to be the boundary here too: the row that closes a group must not draw
// one, or the two read as a single thick line.
test("a group heading is the only line between two groups", async ({ page }) => {
  await page.goto(REGISTRY);
  const periodic = page.locator('main [data-task-group="periodic"]');
  const onDemand = page.locator('main [data-task-group="on_demand"]');

  await expect(periodic.locator(".nf-xrow").last()).toHaveCSS("border-bottom-width", "0px");
  await expect(onDemand.locator(".nf-sec")).toHaveCSS("border-top-width", "1px");
  await expect(onDemand.locator(".nf-sec")).toHaveCSS("border-bottom-width", "1px");
  // The first group's heading leaves the panel's top border alone.
  await expect(periodic.locator(".nf-sec")).toHaveCSS("border-top-width", "0px");
});

// Zero and "no such number" must not look the same: the old page showed
// `0 / 0 / 0` and a dash for a job that had never run at all.
test("a job that has never run shows no counters", async ({ page }) => {
  await page.goto(REGISTRY);
  const never = page
    .locator('main [data-task-kind="job"]')
    .filter({ hasText: "Has not run yet" });
  test.skip((await never.count()) === 0, "every job has already run on this server");
  await expect(never.first()).toContainText("Has not run yet");
  await expect(never.first().locator("[data-counter]")).toHaveCount(0);
});

// The registry columns are what let the jobs be compared down the list: every
// row puts its last run and its duration in the same place, whether that is a
// time, "has not run yet", or a dash for a service.
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

// The lifetime counters are reference data rather than something the list is
// scanned for, so they are one disclosure away — and labelled when they open,
// never a bare triple.
test("a job's counters are labelled behind its disclosure", async ({ page }) => {
  await page.goto(REGISTRY);
  await taskRow(page, "share-link-cleanup")
    .locator('form.trigger-form button[type="submit"]')
    .click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/\/sysadmin\/tasks\/registered\/\?action=triggered$/);

  // A run has to be recorded before the job reports a counter.
  await expect
    .poll(async () => {
      await page.goto(REGISTRY);
      return taskRow(page, "share-link-cleanup").locator("[data-counter]").count();
    })
    .toBeGreaterThan(0);

  const detail = taskRow(page, "share-link-cleanup").locator("details.nf-xrow-more");
  await expect(detail.locator("[data-counter]").first()).toBeHidden();
  await detail.locator("summary").click();
  await expect(detail).toContainText("Totals");
  await expect(detail.locator("[data-counter]").first()).toContainText("Runs");
});

// The policies are a disclosure rather than another line on every row: none of
// them changes what the list is for, and a service has none of them at all.
test("a job's scheduling policy is one disclosure away", async ({ page }) => {
  await page.goto(REGISTRY);
  const row = taskRow(page, "share-link-cleanup");
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
    "History kept",
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

// A job whose subsystem is switched off is not registered at all, which is
// right and also invisible: without this panel it reads as a job the page
// forgot. The default configuration runs no garbage collection.
test("a declared job this server does not run says why", async ({ page }) => {
  await page.goto(REGISTRY);
  await expect(
    page.locator("main .nf-sec h2", { hasText: "Not registered on this server" }),
  ).toBeVisible();
  const gc = page.locator('main [data-task-skipped="gc"]');
  await expect(gc).toContainText("Garbage collection");
  await expect(gc.locator(".nf-prow-hi")).not.toBeEmpty();
  // A reason is a sentence, never the key it was looked up by.
  await expect(page.locator("main [data-task-skipped]").first()).not.toContainText("admin.");
});

// The journal is the durable record, so a run that finished shows up on the run
// list named and labelled rather than as the slug and wire phase the database
// holds.
test("a finished run is named and labelled on the run list", async ({ page }) => {
  await page.goto(REGISTRY);
  await taskRow(page, "share-link-cleanup")
    .locator('form.trigger-form button[type="submit"]')
    .click();
  await page.locator(".js-confirm-ok").click();
  await page.waitForURL(/\/sysadmin\/tasks\/registered\/\?action=triggered$/);

  // The run is queued rather than executed in the request, so the row is only
  // there once the job has finished and the journal has been written.
  await expect
    .poll(async () => {
      await page.goto("/sysadmin/tasks/");
      return page
        .locator('main .nf-xrow[data-run]')
        .filter({ hasText: "Share link cleanup" })
        .count();
    })
    .not.toBe(0);

  const run = page
    .locator('main .nf-xrow[data-run]')
    .filter({ hasText: "Share link cleanup" })
    .first();
  await expect(run.locator(".badge")).toHaveText("Succeeded");
  await expect(run).not.toContainText("share-link-cleanup");
  await expect(run.locator('[data-fact="owner"]')).toContainText("The server");

  // The id and the job's own report are one disclosure away.
  await expect(run.locator("details.nf-xrow-more .nf-kv").first()).toBeHidden();
  await run.locator("details.nf-xrow-more summary").click();
  await expect(run.locator("details.nf-xrow-more")).toContainText("Run id");
});

// The run columns are fixed-width and right-aligned, so the finish times can be
// compared down the list instead of read one row at a time.
test("the run columns line up down the list", async ({ page }) => {
  // Two recorded runs, so there is something to line up across. The first has
  // to reach the journal before the second is submitted, or the job's own
  // concurrency cap turns the duplicate away.
  for (let i = 0; i < 2; i++) {
    await page.goto(REGISTRY);
    await taskRow(page, "share-link-cleanup")
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

// The tab bar carries a bottom margin of its own, and the load card is now the
// first block under it: the two must not touch, or the card reads as part of
// the tab bar.
test("the first block sits clear of the tab bar", async ({ page }) => {
  await page.goto("/sysadmin/tasks/");
  expect(await tabsGap(page)).toBeGreaterThanOrEqual(16);
});

// The load measurements are what explain a run that is waiting, so they come
// first: a run that is holding back the server is read against them rather
// than scrolled past.
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
  const row = taskRow(page, "share-link-cleanup");
  await expect(row).toBeVisible();
  // A periodic job exposes a trigger button; a service has nothing to run.
  await expect(row.locator("form.trigger-form")).toBeVisible();

  // What the trigger has to change is the run the job reports, one disclosure
  // away: the row's visible text is a name, a schedule and a last-run time,
  // which a fast job can leave reading the same as before.
  const runs = async () => {
    await page.goto(REGISTRY);
    const counter = taskRow(page, "share-link-cleanup")
      .locator("details.nf-xrow-more")
      .locator("[data-counter]")
      .first();
    if ((await counter.count()) === 0) return 0;
    await taskRow(page, "share-link-cleanup").locator("details.nf-xrow-more summary").click();
    return Number((await counter.innerText()).replace(/\D+/g, ""));
  };
  const before = await runs();

  await row.locator('form.trigger-form button[type="submit"]').click();
  await page.locator(".js-confirm-ok").click();
  // The button lives on the registry, so the confirmation lands here rather
  // than on the run list.
  await page.waitForURL(/\/sysadmin\/tasks\/registered\/\?action=triggered$/);
  await expect(page.locator("main .nf-banner.is-ok")).toContainText("Task triggered");
  // The banner is a block in its own right: the description has no bottom
  // margin, so the banner has to keep its distance.
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
