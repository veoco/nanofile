import { expect, type Page } from "@playwright/test";

/**
 * The details drawer opens with only its summary row; the rich tier (tags,
 * EXIF, indexed text, share/upload link lists) is collapsed by default and
 * remembered per browser. Specs that assert on those sections must expand it
 * first — this is the one place that knows how.
 */
export async function expandDetails(page: Page): Promise<void> {
  const toggle = page.locator(".js-d-toggle");
  await expect(toggle).toBeVisible();
  if ((await toggle.getAttribute("aria-expanded")) !== "true") {
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
  }
}

/** Selected item's detail row inside the drawer. */
export const details = (page: Page) => page.locator(".js-rp-content");

/** The always-visible "⋯" at the end of a file row. */
export const rowMenuButton = (page: Page, name: string) =>
  page.locator(
    `.js-file-list-view:not(.hidden) .js-entry-row[data-name="${name}"] .nf-more`,
  );

/** Open a row's action menu (which is mounted on <body>, outside the list). */
export async function openRowMenu(page: Page, name: string): Promise<void> {
  await rowMenuButton(page, name).click();
  await expect(page.locator(".nf-menu")).toBeVisible();
}

/**
 * Click a per-row action by name. Rename / delete / share / history / download
 * moved off the row and into its "⋯" menu (the row is a 36px grid row now), so
 * specs open the menu first and then click the item.
 */
export async function clickRowAction(
  page: Page,
  name: string,
  selector: string,
): Promise<void> {
  await openRowMenu(page, name);
  await page.locator(`.nf-menu ${selector}`).first().click();
}
