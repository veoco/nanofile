import { test, expect } from "@playwright/test";
import { readState, createRepo } from "../helpers/api";
import { ADMIN_EMAIL, ADMIN_PASSWORD } from "../helpers/server";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

test("the page marks the browser session in use", async ({ page }) => {
  await page.goto("/settings/credentials/");

  const browsers = page.locator("#browsers");
  await expect(browsers).toContainText("This session");

  // The session in use is the one the run is holding, so the button offers to
  // sign out rather than to revoke something else.
  const current = browsers.locator(".nf-prow").filter({ hasText: "This session" });
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

  await page.goto("/settings/credentials/");
  const section = page.locator("#sync-tokens");
  // The row shows the library's name, not its id.
  const row = section.locator(".nf-prow").filter({ hasText: name });
  await expect(row).toBeVisible();
  await expect(row).toContainText("Never synced");

  await row.locator('form[action$="/revoke/"] button').click();
  await page.locator(".js-confirm-ok").click();
  // Only this token is gone: other specs mint sync tokens for the same account
  // through the sync protocol, so the section may still be there. Once the last
  // unowned token goes the whole section is dropped rather than shown empty,
  // so the row count is what proves the revocation — not the section's text.
  await expect(section.locator(".nf-prow").filter({ hasText: name })).toHaveCount(0);
});

test("a client session that reports no device is still listed", async ({ page }) => {
  // The token minted by `login()` in the helpers carries no device details, and
  // used to be invisible here because the list selected on `platform`.
  await page.goto("/settings/credentials/");
  const clients = page.locator("#devices");
  await expect(clients.locator(".nf-xrow").first()).toBeVisible();
});

test("the leftover sections count exactly the rows they list", async ({ page }) => {
  await page.goto("/settings/credentials/");

  // Both sections only exist while they have something to show: they list the
  // credentials that match no known device, which a normal account has none of.
  // When one is there, its heading number is the number of rows beneath it —
  // the bug this pins rendered the global total over a filtered list.
  const sections = [
    { id: "#sync-tokens", kind: "sync_token" },
    { id: "#device-trusts", kind: "device_trust" },
  ];

  for (const { id, kind } of sections) {
    const section = page.locator(id);
    if ((await section.count()) === 0) continue;

    const heading = Number((await section.locator("h2 span").first().innerText()).trim());
    const rows = await section.locator(`input[name="kind"][value="${kind}"]`).count();
    expect(heading).toBe(rows);
  }
});

test("the summary strip and the device detail both render", async ({ page }) => {
  await page.goto("/settings/credentials/");
  // Four counters, whatever their numbers.
  await expect(page.locator("main").getByText("Browser sessions").first()).toBeVisible();
  await expect(page.locator("main").getByText("Sync tokens").first()).toBeVisible();

  // A device row exposes what unlinking would remove.
  const device = page.locator("#devices .nf-xrow").first();
  if (await device.count()) {
    await device.getByText("Credentials held by this device").click();
    await expect(device).toContainText("Unlinking removes");
  }
});

test("a token a device asks for is shown under it before it ever syncs", async ({ page }) => {
  // A device that identifies itself, the way a desktop client logs in.
  const deviceId = `dev-attr-${Date.now()}`;
  const login = await fetch(`${state.baseURL}/api2/auth-token/`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      username: ADMIN_EMAIL,
      password: ADMIN_PASSWORD,
      platform: "linux",
      device_id: deviceId,
      device_name: "Attributed Device",
      client_version: "3.0.4",
    }),
  });
  if (!login.ok) throw new Error(`device login failed: ${login.status} ${await login.text()}`);
  const { token } = (await login.json()) as { token: string };

  // It receives a repository token but makes no /seafhttp/ request, so the
  // server only knows the device from the request that asked for it.
  const name = `e2e-attributed-${Date.now()}`;
  const repoId = await createRepo(state.baseURL, token, name);
  const res = await fetch(`${state.baseURL}/api2/repo-tokens/?repos=${repoId}`, {
    headers: { authorization: `Bearer ${token}` },
  });
  if (!res.ok) throw new Error(`mint sync token failed: ${res.status} ${await res.text()}`);
  const tokens = (await res.json()) as Record<string, string>;
  expect(tokens[repoId]).toBeTruthy();

  await page.goto("/settings/credentials/");
  const card = page.locator(`[data-device-key="linux:${deviceId}"]`);
  await expect(card).toBeVisible();
  await card.getByText("Credentials held by this device").click();
  await expect(card).toContainText("Sync token");
  await expect(card).toContainText(name);

  // And the token is not filed as unattributed either.
  const leftovers = page.locator("#sync-tokens");
  if ((await leftovers.count()) > 0) {
    await expect(leftovers.locator(".nf-prow").filter({ hasText: name })).toHaveCount(0);
  }
});
