import { test, expect } from "@playwright/test";
import { readState, createRepo } from "../helpers/api";
import { createUserViaAdmin, signInAs } from "../helpers/users";

let state: ReturnType<typeof readState>;

test.beforeAll(async () => {
  state = readState();
});

async function rowAction(
  page: import("@playwright/test").Page,
  row: import("@playwright/test").Locator,
  action: string,
): Promise<void> {
  await row.hover();
  await row.locator(".nf-more").click();
  await page.locator(`.nf-menu [data-action="${action}"]`).click();
}

test("an administrator shares a library with a selected user", async ({ page, browser }) => {
  const repoName = `shared-${Date.now()}`;
  const repoId = await createRepo(state.baseURL, state.adminToken, repoName);
  const reader = await createUserViaAdmin(page, "shared-reader", "reader-password-1");

  await page.goto("/libraries/");
  const row = page.locator("li").filter({ hasText: repoName });
  await expect(row).toBeVisible();

  await rowAction(page, row, "show-share");
  await expect(page.locator("#share-overlay")).toBeVisible();
  await page.locator('input[name="visibility"][value="shared"]').check();
  await page.locator("#share-user-select").selectOption(reader);
  await page.locator("#share-permission-select").selectOption("r");
  await page.locator('[data-action="share-add"]').click();
  await expect(page.locator("#share-member-list")).toContainText(reader);
  await page.locator('#share-form button[type="submit"]').click();

  // The list reloads after a visibility save; the shared user now sees the
  // library with the "Shared with me" badge and no edit/delete menu.
  await page.waitForLoadState("load");
  const readerPage = await signInAs(browser, reader, "reader-password-1");
  try {
    await readerPage.page.goto("/libraries/");
    const sharedRow = readerPage.page.locator("li").filter({ hasText: repoName });
    await expect(sharedRow).toBeVisible();
    await expect(sharedRow.locator(".nf-lib-badge")).toHaveText(/shared/i);
    await sharedRow.hover();
    await sharedRow.locator(".nf-more").click();
    await expect(
      readerPage.page.locator('.nf-menu [data-action="delete-repo"]'),
    ).toHaveCount(0);
  } finally {
    await readerPage.close();
  }

  // API-level cleanup: delete the library as its owner so later specs start clean.
  await fetch(`${state.baseURL}/api2/repos/${repoId}/`, {
    method: "DELETE",
    headers: { authorization: `Token ${state.adminToken}` },
  });
});
