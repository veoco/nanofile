import { test, expect } from "@playwright/test";
import { readState, createRepo, deleteRepo } from "../helpers/api";
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

  // The list reloads after a visibility save. The owner now sees the library's
  // new state on their own row; the shared user sees the "Shared with me" badge
  // and no edit/delete menu.
  await page.waitForLoadState("load");
  await expect(
    page.locator("li").filter({ hasText: repoName }).locator(".nf-lib-badge"),
  ).toHaveText(/shared/i);

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

test("sharing state is visible on the row and filters the list", async ({ page }) => {
  const stamp = Date.now();
  const privateName = `state-private-${stamp}`;
  const publicName = `state-public-${stamp}`;
  const privateId = await createRepo(state.baseURL, state.adminToken, privateName);
  const publicId = await createRepo(state.baseURL, state.adminToken, publicName);
  const setPublic = await fetch(
    `${state.baseURL}/api/v2.1/repos/${publicId}/visibility/`,
    {
      method: "PUT",
      headers: {
        authorization: `Token ${state.adminToken}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ visibility: "public" }),
    },
  );
  expect(setPublic.ok).toBe(true);

  try {
    await page.goto("/libraries/");
    const privateRow = page.locator("li").filter({ hasText: privateName });
    const publicRow = page.locator("li").filter({ hasText: publicName });
    await expect(privateRow).toBeVisible();

    // The owner sees the state on every published row. The icon states it in
    // shape as well: a plain folder for private, a folder with a globe emblem
    // for public.
    await expect(privateRow.locator(".nf-lib-badge")).toHaveCount(0);
    await expect(privateRow.locator(".nf-lib-ic-mark")).toHaveCount(0);
    await expect(publicRow.locator(".nf-lib-badge")).toHaveText("Public");
    await expect(publicRow.locator(".nf-lib-ic-mark")).toHaveCount(1);
    await expect(privateRow.locator(".nf-lib-ic-mark")).toHaveCount(0);

    // The rail repeats the same state: the library tree is not a second,
    // inconsistent rendering of the list.
    const privateRail = page.locator(`.js-repo-item[data-repo-id="${privateId}"]`);
    const publicRail = page.locator(`.js-repo-item[data-repo-id="${publicId}"]`);
    await expect(privateRail.locator(".ic-mark")).toHaveCount(0);
    await expect(publicRail.locator(".ic-mark")).toHaveCount(1);
    // A chip narrows the list, the count follows, and the chip carries the
    // pressed state.
    await page.locator('[data-lib-filter="public"]').click();
    await expect(publicRow).toBeVisible();
    await expect(privateRow).toBeHidden();
    await expect(page.locator('[data-lib-filter="public"]')).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator("#repo-count")).toHaveText("1 library");

    // The chip composes with the search box instead of replacing it.
    await page.locator("#repo-filter").fill(publicName);
    await expect(page.locator("#repo-list > li:visible")).toHaveCount(1);
    await page.locator("#repo-filter").fill("");

    await page.locator('[data-lib-filter="private"]').click();
    await expect(privateRow).toBeVisible();
    await expect(publicRow).toBeHidden();

    await page.locator('[data-lib-filter="all"]').click();
    await expect(privateRow).toBeVisible();
    await expect(publicRow).toBeVisible();
  } finally {
    await deleteRepo(state.baseURL, state.adminToken, privateId);
    await deleteRepo(state.baseURL, state.adminToken, publicId);
  }
});
