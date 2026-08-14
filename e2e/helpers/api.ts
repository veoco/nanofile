import fs from "node:fs";
import path from "node:path";

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
