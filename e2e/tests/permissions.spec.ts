import { test, expect } from "@playwright/test";
import {
  readState,
  createRepo,
  uploadFile,
  createUser,
  beshareRepo,
  loginViaUI,
} from "../helpers/api";

let state: ReturnType<typeof readState>;
let readerEmail: string;
let sharedRepoId: string;
let privateRepoId: string;

test.beforeAll(async () => {
  state = readState();
  readerEmail = `reader-${Date.now()}@test.local`;
  await createUser(state.baseURL, state.adminToken, readerEmail, "password-123");

  // A repo shared read-only with the second user.
  sharedRepoId = await createRepo(state.baseURL, state.adminToken, "shared-to-reader");
  await uploadFile(state.baseURL, state.adminToken, sharedRepoId, "/", "shared.txt", "hello\n");
  await beshareRepo(state.baseURL, state.adminToken, sharedRepoId, readerEmail, "r");

  // An unshared repo the second user must not see.
  privateRepoId = await createRepo(state.baseURL, state.adminToken, "admin-private");
  await uploadFile(state.baseURL, state.adminToken, privateRepoId, "/", "secret.txt", "secret\n");
});

const repoItem = (page: import("@playwright/test").Page, name: string) =>
  page.locator('ul[role="list"] li').filter({ hasText: name });

test("a read-only member sees the shared repo but not unshared ones", async ({ browser }) => {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  try {
    await loginViaUI(page, readerEmail, "password-123");
    await expect(repoItem(page, "shared-to-reader")).toBeVisible();
    await expect(repoItem(page, "admin-private")).toHaveCount(0);

    // The shared repo is browsable.
    await page.goto(`/libraries/${sharedRepoId}/files`);
    await page.waitForSelector(".js-entry-row");
    await expect(
      page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="shared.txt"]'),
    ).toBeVisible();
  } finally {
    await ctx.close();
  }
});

test("a read-only member cannot delete files in the shared repo", async ({ browser }) => {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  try {
    await loginViaUI(page, readerEmail, "password-123");
    await page.goto(`/libraries/${sharedRepoId}/files`);
    await page.waitForSelector(".js-entry-row");
    await page
      .locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="shared.txt"] .js-delete-btn')
      .click();
    await page.locator(".js-confirm-ok").click();
    // The server rejects the write, so the file stays in place in the UI…
    await expect(
      page.locator('.js-file-list-view:not(.hidden) .js-entry-row[data-name="shared.txt"]'),
    ).toBeVisible({ timeout: 15_000 });
    // …and is still served by the API.
    const dir = await page.request.get(`/api2/repos/${sharedRepoId}/dir/?p=/`);
    expect(dir.status()).toBe(200);
    const entries = (await dir.json()) as Array<{ name: string }>;
    expect(entries.some((e) => e.name === "shared.txt")).toBe(true);
  } finally {
    await ctx.close();
  }
});
