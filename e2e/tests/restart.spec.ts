import { test, expect } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
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
 * An isolated instance on its own port, because the restart really does take the
 * server down for a moment and the shared instance serves the rest of the suite.
 *
 * The test is about the *effect*, not the button: a restart-only setting is
 * saved, the button is clicked, and the saved value has to be in force
 * afterwards — which is only true if the server genuinely rebuilt itself.
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
    await page.goto("/sysadmin/settings/general/");

    // A restart-only value this instance leaves to the settings table (the port
    // itself is environment-owned here, so it is not editable).
    const row = page.locator('[data-setting="server.max_json_body_mb"]');
    await expect(row.locator("span.badge", { hasText: "Needs a restart" })).toBeVisible();
    const field = row.locator('input[name="server.max_json_body_mb"]');
    const before = await field.inputValue();
    expect(before).not.toBe("7");
    await field.fill("7");
    await page
      .locator('form[action="/sysadmin/settings/general/save/"]')
      .getByRole("button", { name: "Save", exact: true })
      .click();
    await expect(page.getByText("Settings saved.")).toBeVisible();

    // Saved, and not yet in force: the page says so before the restart.
    await expect(
      page.locator('[data-setting="server.max_json_body_mb"] span.badge', {
        hasText: "Waiting for a restart",
      }),
    ).toBeVisible();

    // Restarting asks first: the button alone must not submit anything.
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

    // The page reloads itself once `/health` answers again.
    await page.waitForURL(/action=restarted/, { timeout: 30_000 });
    await expect(page.getByText("Server restarted.")).toBeVisible();

    // And the value the restart was for is now the running one.
    await expect(
      page.locator('[data-setting="server.max_json_body_mb"] span.badge', {
        hasText: "Waiting for a restart",
      }),
    ).toHaveCount(0);
    await expect(
      page.locator('[data-setting="server.max_json_body_mb"] input[name="server.max_json_body_mb"]'),
    ).toHaveValue("7");

    // The log is where an operator would look, so the restart has to be in it.
    const log = fs.readFileSync(
      path.join(process.cwd(), "test-results", "server.log"),
      "utf-8",
    );
    expect(log).toContain("Restart requested from the admin UI");
    expect(log).toContain("Restarting the server in place");
  } finally {
    await close();
  }
});
