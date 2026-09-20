import { test, expect } from "@playwright/test";
import { mailFor, waitForMail } from "../helpers/mailbox";
import { createUserViaAdmin, signInAs } from "../helpers/users";

/**
 * The three notifications, each scoped to an address this file invents.
 *
 * The decisions being tested are the product ones: a *first* sign-in from a
 * device/browser is announced, a repeat from the same one is not, and a key
 * created through the API is announced the same way as one created in the UI.
 */

test("a client device notifies once, and only the first time", async ({ page }) => {
  const email = await createUserViaAdmin(page, "notify-device", "device-password-123");
  const device = { platform: "linux", device_id: `e2e-device-${Date.now()}` };

  const login = async () => {
    const res = await page.request.post("/api2/auth-token/", {
      form: {
        username: email,
        password: "device-password-123",
        platform: device.platform,
        device_id: device.device_id,
        device_name: "E2E Laptop",
      },
    });
    expect(res.ok()).toBeTruthy();
  };

  await login();
  const mail = await waitForMail({ to: email, subjectIncludes: "New device" });
  expect(mail.body).toContain("E2E Laptop");
  expect(mail.body).toContain("linux");
  expect(mail.body).toContain("remove the device");

  // The same device again is not news. A fixed pause, because this asserts an
  // absence: a notification would take a moment to be delivered, and the check
  // has to outlast that.
  await login();
  await page.waitForTimeout(1500);
  expect(mailFor(email)).toHaveLength(1);

  // A second, different device is.
  device.device_id = `${device.device_id}-other`;
  await login();
  // Delivery is queued, so poll rather than assuming it landed with the request.
  await expect.poll(() => mailFor(email).length, { timeout: 15_000 }).toBe(2);
});

test("a browser sign-in notifies once per browser, not per login", async ({ page, browser }) => {
  const email = await createUserViaAdmin(page, "notify-browser", "browser-password-123");

  const first = await signInAs(browser, email, "browser-password-123");
  await first.close();

  const mail = await waitForMail({ to: email, subjectIncludes: "New sign-in" });
  // The browser is described the way the credentials page describes it, so the
  // reader can match the message against what they see there.
  expect(mail.body).toContain("Chrome");
  expect(mail.body).toContain("Settings -> Credentials");

  // Every context in this run reports the same User-Agent, so this is the same
  // browser: quiet.
  const second = await signInAs(browser, email, "browser-password-123");
  await second.close();
  await page.waitForTimeout(1500);
  expect(mailFor(email)).toHaveLength(1);
});

test("creating an API key from the API notifies the owner", async ({ page, browser }) => {
  const email = await createUserViaAdmin(page, "notify-apikey", "apikey-password-123");
  const session = await signInAs(browser, email, "apikey-password-123");

  // The key is created through the API, which is exactly the path an owner
  // would not otherwise notice.
  const csrf = (await session.page.context().cookies()).find(
    (cookie) => cookie.name === "sfcsrftoken",
  );
  const res = await session.page.request.post("/api2/api-keys/", {
    // A cookie session must echo the CSRF token for any state-changing method,
    // and this endpoint takes JSON.
    headers: {
      "X-CSRFToken": csrf?.value ?? "",
      "content-type": "application/json",
    },
    data: {
      name: "e2e-notified-key",
      capabilities: ["file.read"],
      all_repos: true,
      never: true,
    },
  });
  expect(res.ok()).toBeTruthy();
  await session.close();

  const mail = await waitForMail({ to: email, subjectIncludes: "API key" });
  expect(mail.body).toContain("e2e-notified-key");
  expect(mail.body).toContain("Settings -> API Keys");
});

test("creating an API key in the Web UI notifies the owner too", async ({ page, browser }) => {
  const email = await createUserViaAdmin(page, "notify-keyui", "keyui-password-123");
  const { page: userPage, close } = await signInAs(browser, email, "keyui-password-123");

  await userPage.goto("/settings/api-keys/");
  const form = userPage.locator("#create-key-form");
  await form.locator("#create-key-name").fill("e2e-ui-key");
  await form.locator('input[name="cap__file.read"]').check();
  await form.locator('input[name="all_repos"]').check();
  await form.locator('button[type="submit"]').click();
  await expect(userPage.locator("#new-key-value")).toBeVisible();
  await close();

  const mail = await waitForMail({ to: email, subjectIncludes: "API key" });
  expect(mail.body).toContain("e2e-ui-key");
});
