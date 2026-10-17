import { test, expect } from "@playwright/test";
import fs from "node:fs";
import { signInAs } from "../helpers/users";
import {
  startServer,
  stopServer,
  resolveBinary,
  ADMIN_EMAIL,
  ADMIN_PASSWORD,
} from "../helpers/server";

/**
* Restarting the server from the settings page.
 *
 * An isolated instance on its own port, because the restart really takes the server down. The test
 * is about the effect, not the button: a restart-only setting is saved, the button clicked, and
 * the saved value must be in force afterwards — only true if the server rebuilt itself.
 */
const PORT = 18084;
const URL = `http://127.0.0.1:${PORT}`;

let handle: Awaited<ReturnType<typeof startServer>> | undefined;

test.beforeAll(async () => {
  handle = await startServer(resolveBinary(), { port: PORT });
});

test.afterAll(async () => {
  if (handle) await stopServer(handle);
});

test("the restart button applies a saved restart-only setting", async ({ browser }) => {
  test.setTimeout(60_000);
  const { page, close } = await signInAs(browser, ADMIN_EMAIL, ADMIN_PASSWORD, { baseURL: URL });
  try {
    await page.goto("/sysadmin/settings/server/");

// A restart-only value left to the settings table; the port is environment-owned and not editable.
    const row = page.locator('[data-setting="server.max_json_body_mb"]');
    await expect(row.locator("span.badge", { hasText: "Needs a restart" })).toBeVisible();
    const field = row.locator('input[name="server.max_json_body_mb"]');
    const before = await field.inputValue();
    expect(before).not.toBe("7");
    await field.fill("7");
    await page
      .locator('form[action="/sysadmin/settings/server/save/"]')
      .getByRole("button", { name: "Save", exact: true })
      .click();
    await expect(page.getByText("Settings saved.")).toBeVisible();

    await expect(
      page.locator('[data-setting="server.max_json_body_mb"] span.badge', {
        hasText: "Waiting for a restart",
      }),
    ).toBeVisible();

    const restartButton = page.locator("[data-restart-button]");
    await expect(restartButton).toBeVisible();
    await restartButton.click();
    await expect(page.locator(".js-confirm-ok")).toBeVisible();

    const [response] = await Promise.all([
      page.waitForResponse(
        (r) =>
          r.url().includes("/sysadmin/settings/restart/") && r.request().method() === "POST",
      ),
      page.locator(".js-confirm-ok").click(),
    ]);

    // A late response cannot be asked for its body twice, so read it once we
    // know the request happened.
    expect(response.status()).toBe(200);
    const body = await response.text();
    expect(body).toContain("data-restart-watch");
    expect(body).toContain('data-return="/sysadmin/settings/?action=restarted"');

    await page.waitForURL(/action=restarted/, { timeout: 30_000 });
    await expect(page.getByText("Server restarted.")).toBeVisible();

    await expect(
      page.locator('[data-setting="server.max_json_body_mb"] span.badge', {
        hasText: "Waiting for a restart",
      }),
    ).toHaveCount(0);
    await expect(
      page.locator('[data-setting="server.max_json_body_mb"] input[name="server.max_json_body_mb"]'),
    ).toHaveValue("7");

// The log is where an operator looks, so the restart has to be in it. Each instance logs into
// its own directory, so this path comes from the handle rather than a shared literal.
    const log = fs.readFileSync(handle.logPath, "utf-8");
    expect(log).toContain("Restart requested from the admin UI");
    expect(log).toContain("Restarting the server in place");
  } finally {
    await close();
  }
});
