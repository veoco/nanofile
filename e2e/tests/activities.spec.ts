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
    // The same `Intl` calls core/format.js makes, so a header or tooltip is
    // checked against the reader's own localization rather than a literal.
    const lang = document.documentElement.lang;
    const dateFmt = new Intl.DateTimeFormat(lang, { dateStyle: "medium" });
    const stampFmt = new Intl.DateTimeFormat(lang, { dateStyle: "medium", timeStyle: "medium" });
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
      return {
        key,
        label,
        title: row.querySelector("[data-ts]").title,
        expectedTitle: stampFmt.format(date),
        expectedDateLabel: dateFmt.format(date),
      };
    });
  });

  expect(groups.length).toBeGreaterThan(0);

  // A day header belongs above the first row of each local calendar day and
  // nowhere else: one header per distinct day, never one per row.
  const seen = new Set<string>();
  for (const group of groups) {
    // The row's own stamp spells out the day its header claims, in the reader's
    // timezone.
    expect(group.title).toBe(group.expectedTitle);

    if (seen.has(group.key)) {
      expect(group.label).toBe("");
      continue;
    }
    seen.add(group.key);

    expect(group.label).not.toBe("");
    if (group.label !== "Today" && group.label !== "Yesterday") {
      expect(group.label).toBe(group.expectedDateLabel);
    }
  }
  expect(seen.size).toBeGreaterThan(0);
  // The regression this guards is a header per *row*, so require a day with
  // more than one row: otherwise the loop above proves nothing.
  expect(groups.length).toBeGreaterThan(seen.size);
});

// The day heading is the group boundary, so the row that ends a group draws no
// hairline of its own: the two against each other read as one thick line, and
// the heading's rule beside the label was the second line the reader noticed.
test("a day heading draws the only line above its group", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", `boundary-${Date.now()}.txt`, "x");
  await page.goto("/activities/");
  await page.waitForSelector("main .nf-prow[data-ts-day]");

  // A second local day, without depending on the wall clock or on data seeded a
  // day earlier: the reader's copy of yesterday is the newest row, one day back.
  await page.evaluate(() => {
    const row = document.querySelector("main .nf-prow[data-ts-day]") as HTMLElement;
    const clone = row.cloneNode(true) as HTMLElement;
    clone.dataset.tsDay = String(parseInt(row.dataset.tsDay as string, 10) - 86_400);
    row.parentElement!.appendChild(clone);
  });

  const headings = page.locator("main .nf-list > .nf-sec");
  // The clone's own day opens under a heading the observer inserts for it.
  await expect(headings.last()).toHaveText("Yesterday");
  // The heading's band carries both edges of the boundary …
  await expect(headings.last()).toHaveCSS("border-top-width", "1px");
  await expect(headings.last()).toHaveCSS("border-bottom-width", "1px");
  // … the row it closes draws none of its own …
  await expect(page.locator("main .nf-list > .nf-prow:has(+ .nf-sec)").last()).toHaveCSS(
    "border-bottom-width",
    "0px",
  );
  // … and the first group leaves the panel's own top border to close it.
  await expect(headings.first()).toHaveCSS("border-top-width", "0px");

  // The heading is a surface, not just a label: without it the groups read as
  // one run of rows again.
  const [band, row] = await page.evaluate(() => {
    const bg = (el: Element | null) => (el ? getComputedStyle(el).backgroundColor : "");
    return [
      bg(document.querySelector("main .nf-list > .nf-sec")),
      bg(document.querySelector("main .nf-list > .nf-prow")),
    ];
  });
  expect(band).not.toBe(row);
});

// A list row sheds its metadata column on a phone. The activity row puts the
// class straight on the `[data-ts]` span, so the rule that hides it has to beat
// the `inline-block` a time cell is given — which is why those defaults sit in
// `@layer base` instead of outranking every rule a page can write.
test("a phone row sheds its time column", async ({ page }) => {
  await page.goto("/activities/");
  await page.waitForSelector("main .nf-prow[data-ts-day]");
  const meta = page.locator("main .nf-prow[data-ts-day] .nf-prow-meta").first();
  await expect(meta).toBeVisible();

  await page.setViewportSize({ width: 375, height: 820 });
  await expect(meta).toBeHidden();
});

// The local-day bands are built by core/local-time.js, and its bundle is the
// last thing in <body>. The server ships the feed ungrouped, so the browser
// used to be free to paint it before that bundle ran — and each band (41px of
// layout) then shoved every row below it down. The feed is now held out of the
// first paint until the bands are in place, which this pins by holding the
// bundle back and watching what the browser is allowed to paint.
test("the feed does not shift when the day bands are inserted", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", `shift-${Date.now()}.txt`, "x");

  await page.addInitScript(() => {
    (window as any).__feedShifts = [];
    new PerformanceObserver((list) => {
      for (const e of list.getEntries() as any[]) {
        const inFeed = (e.sources || []).some(
          (s: any) => s.node && s.node.closest && s.node.closest("main .nf-list"),
        );
        if (!e.hadRecentInput && inFeed) (window as any).__feedShifts.push(e.value);
      }
    }).observe({ type: "layout-shift", buffered: true });
  });

  // Hold the bundle: the parsed-but-ungrouped feed is what the browser has in
  // hand. Without the hold the grouping pass usually beats the first paint and
  // the test would prove nothing.
  await page.route("**/static/js/common.*.js", async (route) => {
    await new Promise((r) => setTimeout(r, 2500));
    await route.continue();
  });

  await page.goto("/activities/", { waitUntil: "commit" });
  // `attached`, not the default `visible`: the feed is deliberately held at
  // `visibility: hidden`, so waiting for it to be visible would wait for the
  // very pass this test needs to observe *after* the snapshot below.
  await page.waitForSelector("main .nf-list[data-day-group] .nf-prow", { state: "attached" });

  // Parsed, ungrouped, and held back from paint.
  expect(await page.locator("main .nf-sec").count()).toBe(0);
  await expect(page.locator("main .nf-list[data-day-group]")).toHaveCSS("visibility", "hidden");

  // Once the pass has run the bands exist and the feed is released, so the
  // first painted feed is already grouped.
  await page.waitForSelector("main .nf-list[data-day-grouped]", { timeout: 10_000 });
  expect(await page.locator("main .nf-sec").count()).toBeGreaterThan(0);
  await expect(page.locator("main .nf-list[data-day-group]")).toHaveCSS("visibility", "visible");
  await expect(page.locator("main .nf-prow").first()).toBeVisible();

  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(300);
  expect(await page.evaluate(() => (window as any).__feedShifts)).toEqual([]);
});
