import type { Locator } from "@playwright/test";

/**
 * The stamp `core/format.js` renders for a `[data-ts]` cell, computed in the
 * page with the same `Intl` options and the same language source
 * (`document.documentElement.lang`).
 *
 * Asserting against this rather than a literal keeps the spec independent of
 * the runner's timezone and of the browser's ICU build, while still pinning the
 * localization: a cell that fell back to a numeric or server-shaped stamp would
 * not match.
 *
 *   display  the cell's text — dateStyle "short" + timeStyle "short"
 *   tooltip  the element's `title` — dateStyle "medium" + timeStyle "medium"
 */
export function localStamp(locator: Locator, kind: "display" | "tooltip" = "display"): Promise<string> {
  return locator.evaluate((el, kind) => {
    const ts = Number((el as HTMLElement).dataset.ts);
    const options: Intl.DateTimeFormatOptions =
      kind === "tooltip"
        ? { dateStyle: "medium", timeStyle: "medium" }
        : { dateStyle: "short", timeStyle: "short" };
    return new Intl.DateTimeFormat(document.documentElement.lang, options).format(new Date(ts * 1000));
  }, kind);
}
