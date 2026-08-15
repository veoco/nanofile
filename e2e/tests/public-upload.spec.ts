import { test, expect } from "@playwright/test";
import { readState, seedRepo, createUploadLink } from "../helpers/api";

let state: ReturnType<typeof readState>;
let repoId: string;
let uploadToken: string;
let pwUploadToken: string;

test.beforeAll(async () => {
  state = readState();
  repoId = await seedRepo(state.baseURL, state.adminToken, "public-upload-repo");
  uploadToken = await createUploadLink(state.baseURL, state.adminToken, repoId, "/subdir");
  pwUploadToken = await createUploadLink(state.baseURL, state.adminToken, repoId, "/subdir", "secret");
});

async function listSubdir(): Promise<string[]> {
  const res = await fetch(
    `${state.baseURL}/api2/repos/${repoId}/dir/?p=/subdir`,
    { headers: { authorization: `Bearer ${state.adminToken}` } },
  );
  if (!res.ok) throw new Error(`list subdir failed: ${res.status}`);
  const data = (await res.json()) as Array<{ name: string }>;
  return data.map((d) => d.name);
}

test("upload link uploads a file into the source directory", async ({ page }) => {
  await page.goto(`/u/${uploadToken}/`);
  await expect(page.locator("#drop-zone")).toBeVisible();

  const filename = `from-public-${Date.now()}.txt`;
  await page.setInputFiles("#file-input", {
    name: filename,
    mimeType: "text/plain",
    buffer: Buffer.from("hello from the public upload page\n"),
  });
  await expect(page.locator(".file-item .status.done").first()).toBeVisible({ timeout: 15_000 });

  // The file landed in the shared directory.
  await expect
    .poll(async () => (await listSubdir()).includes(filename), { timeout: 15_000 })
    .toBe(true);
});

test("password-protected upload link requires the password", async ({ page }) => {
  await page.goto(`/u/${pwUploadToken}/`);
  await expect(page.locator('input[name="password"]')).toBeVisible();

  await page.locator('input[name="password"]').fill("wrong");
  await page.locator('button[type="submit"]').click();
  await expect(page.locator(".error")).toContainText("Incorrect password");

  await page.locator('input[name="password"]').fill("secret");
  await page.locator('button[type="submit"]').click();
  await expect(page.locator("#drop-zone")).toBeVisible();
});
