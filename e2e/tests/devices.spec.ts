import { test, expect } from "@playwright/test";
import { readState } from "../helpers/api";
import { ADMIN_EMAIL, ADMIN_PASSWORD } from "../helpers/server";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

test("unlink a device from the credentials page", async ({ page }) => {
  // Create a distinct device via the API so unlinking it doesn't remove the
  // session/API token the other specs rely on.
  const deviceName = `e2e-device-${Date.now()}`;
  const deviceId = `dev-${Date.now()}`;
  const res = await fetch(`${state.baseURL}/api2/auth-token/`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      username: ADMIN_EMAIL,
      password: ADMIN_PASSWORD,
      platform: "linux",
      device_id: deviceId,
      device_name: deviceName,
    }),
  });
  if (!res.ok) throw new Error(`create device token failed: ${res.status} ${await res.text()}`);

  await page.goto("/settings/credentials/");
  const card = page.locator(`[data-device-key="linux:${deviceId}"]`);
  await expect(card).toBeVisible();
  await expect(card).toContainText(deviceName);

  // The device's own credentials are listed inside it, so the owner can see
  // what unlinking removes.
  await card.getByText("Credentials held by this device").click();
  await expect(card).toContainText("Unlinking removes");

  page.once("dialog", (dialog) => dialog.accept());
  await card
    .locator('form[action="/settings/credentials/unlink/"] button[type="submit"]')
    .click();
  await expect(card).toHaveCount(0);
});

test("the old devices URL still leads to the credentials page", async ({ page }) => {
  await page.goto("/settings/devices/");
  await expect(page).toHaveURL(/\/settings\/credentials\/$/);
  await expect(page.locator('nav[aria-label="Settings sections"]')).toBeVisible();
});
