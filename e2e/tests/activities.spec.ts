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

// The day headers are cut in the browser, on the reader's local calendar day: the server ships
// the raw timestamp and core/local-time.js inserts the groups, so header and row cannot disagree.
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

// A day header belongs above the first row of each local day and nowhere else.
  const seen = new Set<string>();
  for (const group of groups) {
// The row's stamp spells out the day its header claims, in the reader's timezone.
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

// The day heading is the group boundary, so the row that ends a group draws no hairline: the two
// against each other would read as one thick line.
test("a day heading draws the only line above its group", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-${Date.now()}`);
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", `boundary-${Date.now()}.txt`, "x");
  await page.goto("/activities/");
  await page.waitForSelector("main .nf-prow[data-ts-day]");

// A second local day without depending on the wall clock: the reader's copy of yesterday is the
// newest row, one day back.
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

// The label is inset 14px on every side and its line box is exactly its font size
// (`leading-none`), which squares its ink against the panel's rounded corner; at the inherited
// 1.5 the 11px label carried 3.9px of leading above the caps against 0.5px beside them. The rule
// also keeps the band 41px rather than 46.5px.
test("a day band insets its label evenly and centers it", async ({ page }) => {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `act-band-${Date.now()}`);
  await uploadFile(state.baseURL, state.adminToken, repoId, "/", `band-${Date.now()}.txt`, "x");
  await page.goto("/activities/");
  await page.waitForSelector("main .nf-prow[data-ts-day]");

// The first band draws no top hairline, so the border it is measured against comes off first.
  const read = () =>
    page.evaluate(() => {
      const el = document.querySelector("main .nf-list > .nf-sec") as HTMLElement;
      const label = el.querySelector("h2") as HTMLElement;
      const b = el.getBoundingClientRect();
      const h = label.getBoundingClientRect();
      const cs = getComputedStyle(el);
      return {
        padLeft: h.left - b.left - parseFloat(cs.borderLeftWidth),
        padTop: h.top - b.top - parseFloat(cs.borderTopWidth),
        // Positive when the label sits below the band's middle.
        drift: h.top + h.height / 2 - (b.top + b.height / 2),
        height: b.height,
      };
    });

  const desktop = await read();
  expect(Math.round(desktop.padLeft)).toBe(14);
  expect(Math.abs(desktop.padTop - desktop.padLeft)).toBeLessThanOrEqual(0.5);
  expect(Math.abs(desktop.drift)).toBeLessThanOrEqual(1);
  // 41px of padding + label + hairlines; the inflated line box would make it 46.5.
  expect(desktop.height).toBeLessThanOrEqual(42);

  // A phone shrinks the band's horizontal inset with the row gutter, so the
  // vertical one has to come down with it or the corner is crooked again — the
  // other way round.
  await page.setViewportSize({ width: 375, height: 820 });
  const phone = await read();
  expect(Math.round(phone.padLeft)).toBe(12);
  expect(Math.abs(phone.padTop - phone.padLeft)).toBeLessThanOrEqual(0.5);
});

// A row sheds its metadata column on a phone. The activity row puts the class on the `[data-ts]`
// span, so the hiding rule must beat the time cell's `inline-block`; those defaults live in
// `@layer base` instead of outranking every page rule.
test("a phone row sheds its time column", async ({ page }) => {
  await page.goto("/activities/");
  await page.waitForSelector("main .nf-prow[data-ts-day]");
  const meta = page.locator("main .nf-prow[data-ts-day] .nf-prow-meta").first();
  await expect(meta).toBeVisible();

  await page.setViewportSize({ width: 375, height: 820 });
  await expect(meta).toBeHidden();
});

// The bands are built by core/local-time.js, whose bundle is last in <body>. The server ships the
// feed ungrouped, so the browser used to paint it first and each 41px band then shoved the rows
// down. The feed is now held out of the first paint until the bands are in place.
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

// Hold the bundle: the parsed-but-ungrouped feed is what the browser has in hand; without the
// hold the grouping pass usually beats the first paint and proves nothing.
  await page.route("**/static/js/common.*.js", async (route) => {
    await new Promise((r) => setTimeout(r, 2500));
    await route.continue();
  });

  await page.goto("/activities/", { waitUntil: "commit" });
// `attached`, not `visible`: the feed is held at `visibility: hidden`, so waiting for visible
// would wait for the very pass this test observes after the snapshot.
  await page.waitForSelector("main .nf-list[data-day-group] .nf-prow", { state: "attached" });

  expect(await page.locator("main .nf-sec").count()).toBe(0);
  await expect(page.locator("main .nf-list[data-day-group]")).toHaveCSS("visibility", "hidden");

// Once the pass has run the bands exist and the feed is released already grouped.
  await page.waitForSelector("main .nf-list[data-day-grouped]", { timeout: 10_000 });
  expect(await page.locator("main .nf-sec").count()).toBeGreaterThan(0);
  await expect(page.locator("main .nf-list[data-day-group]")).toHaveCSS("visibility", "visible");
  await expect(page.locator("main .nf-prow").first()).toBeVisible();

  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(300);
  expect(await page.evaluate(() => (window as any).__feedShifts)).toEqual([]);
});
