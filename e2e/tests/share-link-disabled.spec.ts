import { test, expect } from "@playwright/test";
import { createRepo, login, uploadFile } from "../helpers/api";
import { signInAs } from "../helpers/users";
import { startServer, stopServer, resolveBinary, ADMIN_EMAIL, ADMIN_PASSWORD } from "../helpers/server";

// Sharing is two switches. Each isolated instance binds one of them (or the
// retired master, for the compatibility case) so the shared server used by the
// rest of the suite keeps its defaults.
const DOWNLOAD_PORT = 18083;
const UPLOAD_PORT = 18087;
const LEGACY_PORT = 18088;
const DOWNLOAD_URL = `http://127.0.0.1:${DOWNLOAD_PORT}`;
const UPLOAD_URL = `http://127.0.0.1:${UPLOAD_PORT}`;
const LEGACY_URL = `http://127.0.0.1:${LEGACY_PORT}`;

/** An instance, its admin token, and the library the UI tests browse. */
interface Instance {
  url: string;
  token: string;
  repoId: string;
}

let download: Instance;
let upload: Instance;
let legacy: Instance;
let handles: Awaited<ReturnType<typeof startServer>>[] = [];

// The default storage state belongs to the shared server; cookies are not
// port-scoped, so every instance here signs in on its own.
test.use({ storageState: { cookies: [], origins: [] } });

async function startInstance(
  port: number,
  env: Record<string, string>,
): Promise<Instance> {
  const url = `http://127.0.0.1:${port}`;
  handles.push(await startServer(resolveBinary(), { port, env }));
  const token = await login(url, ADMIN_EMAIL, ADMIN_PASSWORD);
  const repoId = await createRepo(url, token, "switch-lib");
  await uploadFile(url, token, repoId, "/", "shared.txt", "content\n");
  return { url, token, repoId };
}

test.beforeAll(async () => {
  test.setTimeout(120_000);
  download = await startInstance(DOWNLOAD_PORT, {
    NANOFILE_SERVER_SHARE_DOWNLOAD_ENABLED: "false",
  });
  upload = await startInstance(UPLOAD_PORT, {
    NANOFILE_SERVER_SHARE_UPLOAD_ENABLED: "false",
  });
  legacy = await startInstance(LEGACY_PORT, {
    NANOFILE_SERVER_SHARE_LINK_ENABLED: "false",
  });
});

test.afterAll(async () => {
  for (const handle of handles) await stopServer(handle);
  handles = [];
});

test("download sharing off refuses share links but keeps upload links", async ({
  request,
}) => {
  const share = await request.post(`${download.url}/api/v2.1/share-links/`, {
    headers: { authorization: `Bearer ${download.token}`, "content-type": "application/json" },
    data: { repo_id: download.repoId, path: "/shared.txt" },
  });
  expect(share.status()).toBe(403);

  const uploadLink = await request.post(`${download.url}/api/v2.1/upload-links/`, {
    headers: { authorization: `Bearer ${download.token}`, "content-type": "application/json" },
    data: { repo_id: download.repoId, path: "/" },
  });
  expect(uploadLink.status()).toBe(200);

  const info = await request.get(`${download.url}/api2/server-info/`);
  const features = (await info.json()).features as string[];
  expect(features).toContain("share-link-disabled");
});

test("upload sharing off refuses upload links but keeps share links", async ({
  request,
}) => {
  const uploadLink = await request.post(`${upload.url}/api/v2.1/upload-links/`, {
    headers: { authorization: `Bearer ${upload.token}`, "content-type": "application/json" },
    data: { repo_id: upload.repoId, path: "/" },
  });
  expect(uploadLink.status()).toBe(403);

  const share = await request.post(`${upload.url}/api/v2.1/share-links/`, {
    headers: { authorization: `Bearer ${upload.token}`, "content-type": "application/json" },
    data: { repo_id: upload.repoId, path: "/shared.txt" },
  });
  expect(share.status()).toBe(200);

  const info = await request.get(`${upload.url}/api2/server-info/`);
  const features = (await info.json()).features as string[];
  expect(features).not.toContain("share-link-disabled");
});

test("the web UI drops only the controls of the closed direction", async ({
  browser,
}) => {
  const downloadPage = await signInAs(browser, ADMIN_EMAIL, ADMIN_PASSWORD, {
    baseURL: download.url,
  });
  try {
    const page = downloadPage.page;
    await page.goto(`/libraries/${download.repoId}/files/`);
    await expect(page.locator(".js-rp-share")).toHaveCount(0);
    await expect(page.locator("#rp-upload-link-btn")).toHaveCount(1);
    await page.locator(".js-entry-row .nf-more").first().click();
    const menu = page.locator(".nf-menu");
    await expect(menu).toContainText("Get Upload Link");
    await expect(menu).not.toContainText("Get Share Link");
    // The Shares page still manages upload links.
    await page.goto("/shares/");
    await expect(page.locator("#tab-upload-links")).toHaveCount(1);
    await expect(page.locator("#tab-share-links")).toHaveCount(0);
  } finally {
    await downloadPage.close();
  }

  const uploadPage = await signInAs(browser, ADMIN_EMAIL, ADMIN_PASSWORD, {
    baseURL: upload.url,
  });
  try {
    const page = uploadPage.page;
    await page.goto(`/libraries/${upload.repoId}/files/`);
    await expect(page.locator(".js-rp-share")).toHaveCount(1);
    await expect(page.locator("#rp-upload-link-btn")).toHaveCount(0);
    await page.locator(".js-entry-row .nf-more").first().click();
    const menu = page.locator(".nf-menu");
    await expect(menu).toContainText("Get Share Link");
    await expect(menu).not.toContainText("Get Upload Link");
    await page.goto("/shares/");
    await expect(page.locator("#tab-share-links")).toHaveCount(1);
    await expect(page.locator("#tab-upload-links")).toHaveCount(0);
  } finally {
    await uploadPage.close();
  }
});

test("the retired master switch still closes both directions", async ({ request }) => {
  for (const path of [
    "/api/v2.1/share-links/",
    "/api/v2.1/upload-links/",
  ]) {
    const res = await request.post(`${legacy.url}${path}`, {
      headers: { authorization: `Bearer ${legacy.token}`, "content-type": "application/json" },
      data: { repo_id: legacy.repoId, path: "/shared.txt" },
    });
    expect(res.status(), `${path} must be refused`).toBe(403);
  }

  const info = await request.get(`${legacy.url}/api2/server-info/`);
  const features = (await info.json()).features as string[];
  expect(features).toContain("share-link-disabled");
});

test("the retired variable is shown as the origin on the settings page", async ({
  browser,
}) => {
  const session = await signInAs(browser, ADMIN_EMAIL, ADMIN_PASSWORD, {
    baseURL: legacy.url,
  });
  try {
    const page = session.page;
    await page.goto("/sysadmin/settings/security/");
    for (const flag of ["share_download_enabled", "share_upload_enabled"]) {
      const row = page.locator(`[data-setting="server.${flag}"]`);
      await expect(row).toContainText("Environment");
      await expect(row).toContainText("NANOFILE_SERVER_SHARE_LINK_ENABLED");
    }
    // Both halves are off, so the shell drops the Shares link and the page 404s.
    await expect(page.locator('a[href="/shares/"]')).toHaveCount(0);
    await page.goto("/shares/");
    await expect(page.getByText("Page not found")).toBeVisible();
  } finally {
    await session.close();
  }
});
