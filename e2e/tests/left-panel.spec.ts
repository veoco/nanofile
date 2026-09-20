import { test, expect } from "@playwright/test";

// The rail sits beside the main column and pins a few fixed blocks above its
// footer. Anything in it that changes size after load — the storage meter is
// filled by fetch() — moves its neighbours, which is what a page load used to
// look like: the Browse block and the tree's lower edge jumped up by the
// meter's height once the request landed.
//
// The meter must hold its box as well as its row. The layout-shift API alone
// cannot police that: a move below three pixels is under its reporting
// threshold, and the row used to grow by half a pixel when the value landed
// (an empty value is a zero-height flex item, so baseline alignment added its
// descent only once it had text). So the account response is held back and the
// rail's geometry is compared before and after it arrives, exactly.
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

  // Hold the meter's data so the first snapshot is the pre-response one.
  await page.route("**/api2/account/info/**", async (route) => {
    await new Promise((r) => setTimeout(r, 1500));
    await route.continue();
  });

  await page.goto("/libraries/", { waitUntil: "domcontentloaded" });

  const railGeometry = () =>
    page.evaluate(() => {
      const rect = (el: Element | null) => {
        if (!el) throw new Error("rail block not found");
        const r = el.getBoundingClientRect();
        return { top: r.top, bottom: r.bottom, height: r.height, width: r.width };
      };
      const storage = document.querySelector("#nf-storage");
      return {
        storage: rect(storage),
        track: rect(document.querySelector("#nf-storage-track")),
        browse: rect(storage && storage.previousElementSibling),
        tree: rect(document.querySelector(".js-repo-tree")),
      };
    });

  // The row is in the server-rendered markup, so it holds its space before the
  // account request resolves; the request only fills the value in. The track
  // stays visible even for an unlimited quota, which would otherwise make the
  // row's height depend on the response.
  expect(await page.locator("#nf-storage-text").textContent()).toBe("");
  const before = await railGeometry();

  await expect(page.locator("#nf-storage")).toBeVisible();
  await expect(page.locator("#nf-storage-track")).toBeVisible();
  await expect(page.locator("#nf-storage-text")).toHaveText(/\d/);

  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(300);

  expect(await railGeometry()).toEqual(before);

  expect(await page.evaluate(() => (window as any).__railShifts)).toEqual([]);
});
