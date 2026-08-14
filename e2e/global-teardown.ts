import fs from "node:fs";
import path from "node:path";

export default async function globalTeardown() {
  const p = path.join(process.cwd(), "test-results", ".server.json");
  try {
    const info = JSON.parse(fs.readFileSync(p, "utf-8")) as { pid: number; tmpRoot: string };
    try {
      process.kill(info.pid, "SIGTERM");
    } catch {
      // process already gone
    }
    fs.rmSync(info.tmpRoot, { recursive: true, force: true });
  } catch {
    // nothing to clean up
  }
}
