import { test, expect } from "@playwright/test";
import { readState } from "../helpers/api";
import { ADMIN_EMAIL, ADMIN_PASSWORD } from "../helpers/server";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

test("unlink a device from the devices page", async ({ page }) => {
  // Create a distinct device via the API so unlinking it doesn't remove the
  // session/API token the other specs rely on.
  const deviceName = `e2e-device-${Date.now()}`;
  const res = await fetch(`${state.baseURL}/api2/auth-token/`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      username: ADMIN_EMAIL,
      password: ADMIN_PASSWORD,
      platform: "linux",
      device_id: `dev-${Date.now()}`,
      device_name: deviceName,
    }),
  });
  if (!res.ok) throw new Error(`create device token failed: ${res.status} ${await res.text()}`);

  await page.goto("/settings/devices/");
  const card = page.locator("main .rounded-lg").filter({ hasText: deviceName });
  await expect(card).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await card.locator('form button[type="submit"]').click();
  await expect(card).toHaveCount(0);
});
