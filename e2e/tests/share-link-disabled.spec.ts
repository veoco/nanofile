import { test, expect } from "@playwright/test";
import { login } from "../helpers/api";
import { startServer, stopServer, resolveBinary, ADMIN_EMAIL, ADMIN_PASSWORD } from "../helpers/server";

// Isolated instance bound to the `share_link_enabled = false` switch so we can
// verify the global disable behaviour without affecting the shared server used
// by the rest of the suite.
const DISABLED_PORT = 18083;
const DISABLED_URL = `http://127.0.0.1:${DISABLED_PORT}`;

let handle: Awaited<ReturnType<typeof startServer>> | undefined;
let adminToken: string;

test.beforeAll(async () => {
  handle = await startServer(resolveBinary(), {
    port: DISABLED_PORT,
    env: { NANOFILE_SERVER_SHARE_LINK_ENABLED: "false" },
  });
  adminToken = await login(DISABLED_URL, ADMIN_EMAIL, ADMIN_PASSWORD);
});

test.afterAll(async () => {
  if (handle) await stopServer(handle);
});

test("server-info advertises share-link-disabled when the switch is off", async ({ request }) => {
  const res = await request.get(`${DISABLED_URL}/api2/server-info/`);
  expect(res.status()).toBe(200);
  const body = await res.json();
  const features = body.features as string[];
  expect(features).toContain("share-link-disabled");
});

test("creating a share link returns 403 when links are disabled", async ({ request }) => {
  const res = await request.post(`${DISABLED_URL}/api/v2.1/share-links/`, {
    headers: { authorization: `Bearer ${adminToken}`, "content-type": "application/json" },
    data: { repo_id: "some-repo", path: "/" },
  });
  expect(res.status()).toBe(403);
});

test("creating an upload link returns 403 when links are disabled", async ({ request }) => {
  const res = await request.post(`${DISABLED_URL}/api/v2.1/upload-links/`, {
    headers: { authorization: `Bearer ${adminToken}`, "content-type": "application/json" },
    data: { repo_id: "some-repo", path: "/" },
  });
  expect(res.status()).toBe(403);
});
