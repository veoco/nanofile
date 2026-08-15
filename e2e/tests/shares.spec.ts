import { test, expect } from "@playwright/test";
import { readState, seedRepo } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "shares-repo");
});

async function createShareLink(path: string, description: string): Promise<void> {
  const res = await fetch(`${state.baseURL}/api/v2.1/share-links/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${state.adminToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ repo_id: repoId, path, description }),
  });
  if (!res.ok) {
    throw new Error(`create share link failed: ${res.status} ${await res.text()}`);
  }
}

async function createUploadLink(path: string, description: string): Promise<void> {
  const res = await fetch(`${state.baseURL}/api/v2.1/upload-links/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${state.adminToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ repo_id: repoId, path, description }),
  });
  if (!res.ok) {
    throw new Error(`create upload link failed: ${res.status} ${await res.text()}`);
  }
}

const shareRow = (page: import("@playwright/test").Page, text: string) =>
  page.locator("#tab-share-links tbody tr").filter({ hasText: text });

const uploadRow = (page: import("@playwright/test").Page, text: string) =>
  page.locator("#tab-upload-links tbody tr").filter({ hasText: text });

test("shares page lists share and upload links in their tabs", async ({ page }) => {
  const shareDesc = `share-desc-${Date.now()}`;
  const uploadDesc = `upload-desc-${Date.now()}`;
  await createShareLink("/alpha.txt", shareDesc);
  await createUploadLink("/subdir", uploadDesc);

  await page.goto("/shares/");
  await expect(shareRow(page, shareDesc)).toBeVisible();
  await page.locator('[data-tab="upload-links"]').click();
  await expect(page.locator("#tab-upload-links")).toBeVisible();
  await expect(page.locator("#tab-share-links")).toBeHidden();
  await expect(uploadRow(page, uploadDesc)).toBeVisible();
});

test("edit a share link description", async ({ page }) => {
  const original = `orig-desc-${Date.now()}`;
  const updated = `updated-desc-${Date.now()}`;
  await createShareLink("/bravo.txt", original);
  await page.goto("/shares/");
  const row = shareRow(page, original);
  await row.locator(".js-share-edit-btn").click();
  await expect(page.locator("#share-edit-overlay")).toBeVisible();
  await page.locator("#edit-description-input").fill(updated);
  await page.locator(".js-edit-confirm").click();
  // Saving reloads the page; the updated description replaces the original.
  await expect(shareRow(page, updated)).toBeVisible({ timeout: 15_000 });
  await expect(shareRow(page, original)).toHaveCount(0);
});

test("delete a share link", async ({ page }) => {
  const desc = `del-desc-${Date.now()}`;
  await createShareLink("/charlie.txt", desc);
  await page.goto("/shares/");
  const row = shareRow(page, desc);
  await expect(row).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await row.locator('form.delete-form button[type="submit"]').click();
  await expect(row).toHaveCount(0, { timeout: 15_000 });
});
