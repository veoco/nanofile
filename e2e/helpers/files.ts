import { type Page } from "@playwright/test";
import { seedRepo } from "./api";

/** Open a library's file browser and wait for its first row. */
export async function openRepo(page: Page, repoId: string): Promise<void> {
  await page.goto(`/libraries/${repoId}/files`);
  await page.waitForSelector(".js-entry-row");
}

/**
 * Seed a fresh library and open it in one step.
 */
export async function seedAndOpenRepo(
  page: Page,
  state: { baseURL: string; adminToken: string },
  prefix: string,
): Promise<string> {
  const repoId = await seedRepo(state.baseURL, state.adminToken, `${prefix}-${Date.now()}`);
  await openRepo(page, repoId);
  return repoId;
}