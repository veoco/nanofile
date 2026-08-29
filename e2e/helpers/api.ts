import fs from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";

export interface E2EState {
  baseURL: string;
  adminEmail: string;
  adminPassword: string;
  adminToken: string;
}

export function readState(): E2EState {
  const p = path.join(process.cwd(), "test-results", ".e2e-state.json");
  return JSON.parse(fs.readFileSync(p, "utf-8")) as E2EState;
}

/** POST /api2/auth-token/ → bearer token (used by API helpers; no CSRF needed). */
export async function login(baseURL: string, username: string, password: string): Promise<string> {
  const res = await fetch(`${baseURL}/api2/auth-token/`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ username, password }),
  });
  if (!res.ok) throw new Error(`login failed: ${res.status} ${await res.text()}`);
  const data = (await res.json()) as { token: string };
  return data.token;
}

/** POST /api2/repos/ → repo id (string). */
export async function createRepo(baseURL: string, token: string, name: string): Promise<string> {
  const res = await fetch(`${baseURL}/api2/repos/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/x-www-form-urlencoded",
    },
    body: new URLSearchParams({ name }),
  });
  if (!res.ok) throw new Error(`create repo failed: ${res.status} ${await res.text()}`);
  const data = (await res.json()) as { id: string };
  return data.id;
}

/**
 * Create an encrypted repo (enc_version 2) and return its repo id.
 *
 * The magic / random_key are derived client-side exactly as the Rust
 * `infra::crypto::key_derivation` module does for v2 (fixed MAGIC_SALT):
 *   magic      = PBKDF2-SHA256(repo_id + password, MAGIC_SALT, 1000) → 32B hex
 *   derivedKey = PBKDF2-SHA256(password, MAGIC_SALT, 1000) → 32B
 *   derivedIv  = PBKDF2-SHA256(derivedKey, MAGIC_SALT, 10) → 16B
 *   random_key = AES-256-CBC-encrypt(32B secret, derivedKey, derivedIv) + PKCS7 → 48B hex
 */
export async function createEncryptedRepo(
  baseURL: string,
  token: string,
  name: string,
  password: string,
): Promise<string> {
  const crypto = await import("node:crypto");
  const MAGIC_SALT = Buffer.from([0xda, 0x90, 0x45, 0xc3, 0x06, 0xc7, 0xcc, 0x26]);
  const repoId = crypto.randomUUID();

  const magic = crypto
    .pbkdf2Sync(Buffer.from(`${repoId}${password}`), MAGIC_SALT, 1000, 32, "sha256")
    .toString("hex");

  const derivedKey = crypto.pbkdf2Sync(Buffer.from(password), MAGIC_SALT, 1000, 32, "sha256");
  const derivedIv = crypto.pbkdf2Sync(derivedKey, MAGIC_SALT, 10, 16, "sha256");
  const secret = crypto.randomBytes(32);
  const cipher = crypto.createCipheriv("aes-256-cbc", derivedKey, derivedIv);
  const randomKey = Buffer.concat([cipher.update(secret), cipher.final()]).toString("hex");

  const res = await fetch(`${baseURL}/api2/repos/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/x-www-form-urlencoded",
    },
    body: new URLSearchParams({
      name,
      repo_id: repoId,
      encrypted: "1",
      enc_version: "2",
      magic,
      random_key: randomKey,
    }),
  });
  if (!res.ok) throw new Error(`create encrypted repo failed: ${res.status} ${await res.text()}`);
  return repoId;
}

/** POST /api2/repos/{id}/file/ (multipart) → upload a file. */
export async function uploadFile(
  baseURL: string,
  token: string,
  repoId: string,
  parentDir: string,
  filename: string,
  content: string | Uint8Array,
): Promise<void> {
  const form = new FormData();
  form.append("file", new Blob([content]), filename);
  form.append("parent_dir", parentDir);
  form.append("replace", "1");
  const res = await fetch(`${baseURL}/api2/repos/${repoId}/file/`, {
    method: "POST",
    headers: { authorization: `Bearer ${token}` },
    body: form,
  });
  if (!res.ok) throw new Error(`upload ${filename} failed: ${res.status} ${await res.text()}`);
}

/** POST /api2/repos/{id}/dir/ (JSON {p}) → create a directory. */
export async function createDir(
  baseURL: string,
  token: string,
  repoId: string,
  p: string,
): Promise<void> {
  const res = await fetch(`${baseURL}/api2/repos/${repoId}/dir/`, {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify({ p }),
  });
  if (!res.ok) throw new Error(`mkdir ${p} failed: ${res.status} ${await res.text()}`);
}

/**
 * Create a repo seeded with a standard mix of entries for browser tests:
 * several text files (for selection/sorting), one directory with a nested
 * file (for navigation), and one previewable text file.
 */
export async function seedRepo(
  baseURL: string,
  token: string,
  name = "e2e-repo",
): Promise<string> {
  const repoId = await createRepo(baseURL, token, name);
  const files = ["alpha.txt", "bravo.txt", "charlie.txt", "delta.txt"];
  for (let i = 0; i < files.length; i++) {
    await uploadFile(baseURL, token, repoId, "/", files[i], `content of ${files[i]} (${i + 1})\n`);
  }
  await createDir(baseURL, token, repoId, "/subdir");
  await uploadFile(baseURL, token, repoId, "/subdir", "nested.txt", "nested file content\n");
  return repoId;
}

/** Sign in via the UI login form (for fresh, non-storageState contexts). */
export async function loginViaUI(page: Page, email: string, password: string): Promise<void> {
  await page.goto("/accounts/login/");
  await page.fill('input[name="email"]', email);
  await page.fill('input[name="password"]', password);
  await page.locator('button[type="submit"]').first().click();
  await page.waitForURL(/\/libraries\//, { timeout: 15_000 });
}

/** POST /api/v2.1/share-links/ → the created link's token. */
export async function createShareLink(
  baseURL: string,
  token: string,
  repoId: string,
  path: string,
  password?: string,
): Promise<string> {
  const body: Record<string, string> = { repo_id: repoId, path };
  if (password) body.password = password;
  const res = await fetch(`${baseURL}/api/v2.1/share-links/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`create share link failed: ${res.status} ${await res.text()}`);
  return (await res.json()).token;
}

/** POST /api/v2.1/upload-links/ → the created link's token. */
export async function createUploadLink(
  baseURL: string,
  token: string,
  repoId: string,
  path: string,
  password?: string,
): Promise<string> {
  const body: Record<string, string> = { repo_id: repoId, path };
  if (password) body.password = password;
  const res = await fetch(`${baseURL}/api/v2.1/upload-links/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`create upload link failed: ${res.status} ${await res.text()}`);
  return (await res.json()).token;
}

/** POST /api2/accounts/ — admin-only: create a new (non-admin, active) user. */
export async function createUser(
  baseURL: string,
  adminToken: string,
  email: string,
  password: string,
): Promise<void> {
  const res = await fetch(`${baseURL}/api2/accounts/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${adminToken}`,
      "content-type": "application/x-www-form-urlencoded",
    },
    body: new URLSearchParams({ email, password }),
  });
  if (!res.ok) throw new Error(`create user ${email} failed: ${res.status} ${await res.text()}`);
}

/** POST /api2/beshared-repos/{repoId}/ — share a repo with another user. */
export async function beshareRepo(
  baseURL: string,
  token: string,
  repoId: string,
  userEmail: string,
  permission: "r" | "rw" = "rw",
): Promise<void> {
  const res = await fetch(`${baseURL}/api2/beshared-repos/${repoId}/`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ share_type: "personal", user: userEmail, permission }),
  });
  if (!res.ok) {
    throw new Error(`beshare ${repoId} to ${userEmail} failed: ${res.status} ${await res.text()}`);
  }
}
