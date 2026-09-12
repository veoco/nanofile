import { test, expect } from "@playwright/test";
import { readState, createRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

test("the page marks the browser session in use", async ({ page }) => {
  await page.goto("/settings/devices/");

  const browsers = page.locator("main section").filter({ hasText: "Browser sessions" });
  await expect(browsers).toContainText("This session");

  // The session in use is the one the run is holding, so the button offers to
  // sign out rather than to revoke something else.
  const current = browsers.locator(".card").filter({ hasText: "This session" });
  await expect(current.locator('form[action$="/revoke/"] button')).toHaveText("Sign out");
});

test("a sync token is visible and can be revoked", async ({ page }) => {
  // Sync tokens are minted by the sync protocol, not by logging in, so seed one
  // the way a desktop client would.
  const name = `e2e-sync-${Date.now()}`;
  const repoId = await createRepo(state.baseURL, state.adminToken, name);
  const res = await fetch(`${state.baseURL}/api2/repo-tokens/?repos=${repoId}`, {
    headers: { authorization: `Bearer ${state.adminToken}` },
  });
  if (!res.ok) throw new Error(`mint sync token failed: ${res.status} ${await res.text()}`);
  const tokens = (await res.json()) as Record<string, string>;
  expect(Object.keys(tokens)).toContain(repoId);

  await page.goto("/settings/devices/");
  const section = page.locator("main section").filter({ hasText: "Sync tokens" });
  // The card shows the library's name, not its id.
  const card = section.locator(".card").filter({ hasText: name });
  await expect(card).toBeVisible();
  await expect(card).toContainText("Never synced");

  page.once("dialog", (dialog) => dialog.accept());
  await card.locator('form[action$="/revoke/"] button').click();
  // Only this token is gone: other specs mint sync tokens for the same account
  // through the sync protocol, so the section is not necessarily empty.
  await expect(card).toHaveCount(0);
});

test("a client session that reports no device is still listed", async ({ page }) => {
  // The token minted by `login()` in the helpers carries no device details, and
  // used to be invisible here because the list selected on `platform`.
  await page.goto("/settings/devices/");
  const clients = page.locator("main section").filter({ hasText: "Client sessions" });
  await expect(clients.locator(".card").first()).toBeVisible();
});
