import fs from "node:fs";
import path from "node:path";

/**
 * The server's warning for a request that reached a route nobody classified
 * (`server/src/middleware/route_audit.rs`). Such a request is already a 403 —
 * the route table is fail-closed — so this gate is not about the response: it
 * is about the *gap*. A registered endpoint missing from the table is a bug
 * that otherwise shows up as one line in a log nobody reads.
 */
const UNCLASSIFIED_ROUTE_WARNING = "credential used on a route with no classification";

export default async function globalTeardown() {
  const resultsDir = path.join(process.cwd(), "test-results");
  const p = path.join(resultsDir, ".server.json");
  try {
    const info = JSON.parse(fs.readFileSync(p, "utf-8")) as { pid: number; tmpRoot: string };
    try {
      // Every platform, one spelling: `SIGTERM` is a real signal only on unix,
      // and Node rejects the name on Windows, where this is `TerminateProcess`
      // either way. The teardown cannot make its own kill graceful, so it does
      // not pretend to.
      process.kill(info.pid);
      await waitForExit(info.pid);
    } catch {
      // process already gone
    }
    fs.rmSync(info.tmpRoot, { recursive: true, force: true });
  } catch {
    // nothing to clean up
  }

  assertNoUnclassifiedRoutes(resultsDir);
}

/**
 * Wait for a process to disappear. Each instance's log is truncated (or, for
 * the isolated specs, freshly created) at startup, so the audit below reads only
 * this run — but the last writes still have to land before the logs are read.
 */
async function waitForExit(pid: number, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      process.kill(pid, 0);
    } catch {
      return;
    }
    await new Promise<void>((resolve) => setTimeout(resolve, 50));
  }
}

/**
 * Audit every server instance's log, not just the shared one.
 *
 * Three specs spawn their own isolated server, so a single path here would miss
 * whatever they reached — and the gap this gate looks for is exactly the kind
 * that only one spec would hit. Each instance logs into
 * `test-results/<tmpRoot>/server.log` (see `helpers/server.ts`).
 */
function assertNoUnclassifiedRoutes(resultsDir: string): void {
  const logs = serverLogs(resultsDir);
  if (logs.length === 0) {
    return; // no log to audit
  }

  const hits: string[] = [];
  for (const log of logs) {
    const lines = log
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.includes(UNCLASSIFIED_ROUTE_WARNING));
    hits.push(...lines);
  }

  if (hits.length > 0) {
    throw new Error(
      `The server reached ${hits.length} route(s) that are missing from the capability table ` +
        `(server/src/domain/capability.rs).\n` +
        `Every route a credential can reach must be classified, because an unclassified route ` +
        `answers 403 to everyone:\n  ${hits.join("\n  ")}`,
    );
  }
}

/** Every `server.log` an instance of this run wrote, oldest directory first. */
function serverLogs(resultsDir: string): string[] {
  const logs: string[] = [];
  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(resultsDir, { withFileTypes: true });
  } catch {
    return logs;
  }
  for (const entry of entries
    .filter((entry) => entry.isDirectory() && entry.name.startsWith("nanofile-e2e-"))
    .sort((a, b) => a.name.localeCompare(b.name))) {
    const log = path.join(resultsDir, entry.name, "server.log");
    try {
      logs.push(fs.readFileSync(log, "utf-8"));
    } catch {
      // this instance wrote no log, which is its own problem and not this one
    }
  }
  return logs;
}
