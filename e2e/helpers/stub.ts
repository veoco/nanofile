import { type Page } from "@playwright/test";

/** Replace navigator.clipboard with a recorder for specs that assert copying. */
export async function stubClipboard(page: Page): Promise<void> {
  await page.addInitScript(() => {
    (window as any).__copied = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          (window as any).__copied.push(text);
        },
      },
    });
  });
}