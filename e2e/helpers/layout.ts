import { expect, type Page } from "@playwright/test";

/**
 * Vertical gap between two elements of the page.
 *
 * `.page-subtitle` carries only a top margin, so the gap under it is whatever
 * the next block contributes: a panel or banner with no top margin sits flush
 * against the description. Pages whose shape is title → subtitle → next block
 * must therefore put the spacing on that next block.
 */
async function gapBetween(page: Page, from: string, to: string): Promise<number> {
  const gap = await page.evaluate(
    ([fromSel, toSel]) => {
      const above = document.querySelector(fromSel);
      const below = document.querySelector(toSel);
      if (!above || !below) return null;
      return below.getBoundingClientRect().top - above.getBoundingClientRect().bottom;
    },
    [from, to],
  );
  expect(gap, `page needs both ${from} and ${to} to measure`).not.toBeNull();
  return gap as number;
}

/** Gap between the page description and the panel below it. */
export function subtitleGap(page: Page): Promise<number> {
  return gapBetween(page, "main .page-subtitle", "main .nf-list");
}

/** Gap between the page description and the first banner below it. */
export function bannerGap(page: Page): Promise<number> {
  return gapBetween(page, "main .page-subtitle", "main .nf-banner");
}

/**
 * Gap between the tab bar and the first block below it.
 *
 * A page with tabs does not put its first panel straight under the description,
 * so `subtitleGap` measures across the tabs and says nothing about whether the
 * two touch.
 */
export function tabsGap(page: Page): Promise<number> {
  return gapBetween(page, "main .nf-tabs", "main [data-panel]");
}
