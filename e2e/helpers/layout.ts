import { expect, type Page } from "@playwright/test";

/**
 * Vertical gap between the page subtitle and the panel below it.
 *
 * `.page-subtitle` carries only a top margin, so the gap under it is whatever
 * the next block contributes: a `.nf-list` with no top margin sits flush
 * against the description. Pages whose shape is title → subtitle → panel must
 * therefore put the spacing on the panel.
 */
export async function subtitleGap(page: Page): Promise<number> {
  const gap = await page.evaluate(() => {
    const subtitle = document.querySelector("main .page-subtitle");
    const panel = document.querySelector("main .nf-list");
    if (!subtitle || !panel) return null;
    return panel.getBoundingClientRect().top - subtitle.getBoundingClientRect().bottom;
  });
  expect(gap, "page needs a subtitle and a panel to measure").not.toBeNull();
  return gap as number;
}
