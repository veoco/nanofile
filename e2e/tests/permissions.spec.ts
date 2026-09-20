import { test, expect } from "@playwright/test";
import { readState, createRepo, uploadFile, createUser, loginViaUI } from "../helpers/api";

/**
 * Libraries are per-account: there is no share-to-user surface any more, so the
 * only cross-account assertion left is isolation. A second account must see
 * none of the first account's libraries and must be refused on their API.
 */

let state: ReturnType<typeof readState>;
let otherEmail: string;
let privateRepoId: string;

test.beforeAll(async () => {
  state = readState();
  otherEmail = `other-${Date.now()}@test.local`;
  await createUser(state.baseURL, state.adminToken, otherEmail, "password-123");

  privateRepoId = await createRepo(state.baseURL, state.adminToken, "admin-private");
  await uploadFile(state.baseURL, state.adminToken, privateRepoId, "/", "secret.txt", "secret\n");
});

const repoItem = (page: import("@playwright/test").Page, name: string) =>
  page.locator('ul[role="list"] li').filter({ hasText: name });

test("a second account sees none of the first account's libraries", async ({ browser }) => {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  try {
    await loginViaUI(page, otherEmail, "password-123");

    await expect(repoItem(page, "admin-private")).toHaveCount(0);

    // The API agrees: the other account's library list does not include it.
    const resp = await page.request.get("/api2/repos/");
    expect(resp.status()).toBe(200);
    const repos = (await resp.json()) as Array<{ id: string }>;
    expect(repos.some((r) => r.id === privateRepoId)).toBe(false);
  } finally {
    await ctx.close();
  }
});

test("a second account is refused on another account's library", async ({ browser }) => {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  try {
    await loginViaUI(page, otherEmail, "password-123");

    const dir = await page.request.get(`/api2/repos/${privateRepoId}/dir/?p=/`);
    expect(dir.status()).toBe(403);

    // The web UI does not render the entries either.
    await page.goto(`/libraries/${privateRepoId}/files`);
    await expect(
      page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="secret.txt"]'),
    ).toHaveCount(0);
  } finally {
    await ctx.close();
  }
});
