import { test, expect } from "@playwright/test";

// The rail sits beside the main column and pins a few fixed blocks above its
// footer. Anything in it that changes size after load — the storage meter is
// filled by fetch() — moves its neighbours, which is what a page load used to
// look like: the Browse block and the tree's lower edge jumped up by the
// meter's height once the request landed.
test("the rail holds its layout while the storage meter loads", async ({ page }) => {
  await page.addInitScript(() => {
    const inRail = (e: any) =>
      e.sources.some(
        (s: any) => s.node && s.node.closest && s.node.closest(".js-left-panel"),
      );
    (window as any).__railShifts = [];
    new PerformanceObserver((list) => {
      for (const e of list.getEntries() as any[]) {
        if (!e.hadRecentInput && inRail(e)) (window as any).__railShifts.push(e.value);
      }
    }).observe({ type: "layout-shift", buffered: true });
  });

  await page.goto("/libraries/");

  // The row is in the server-rendered markup, so it holds its space before the
  // account request resolves; the request only fills the value in. The track
  // stays visible even for an unlimited quota, which would otherwise make the
  // row's height depend on the response.
  await expect(page.locator("#nf-storage")).toBeVisible();
  await expect(page.locator("#nf-storage-track")).toBeVisible();
  await expect(page.locator("#nf-storage-text")).toHaveText(/\d/);

  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(300);

  expect(await page.evaluate(() => (window as any).__railShifts)).toEqual([]);
});
