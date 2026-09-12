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
      process.kill(info.pid, "SIGTERM");
      await waitForExit(info.pid);
    } catch {
      // process already gone
    }
    fs.rmSync(info.tmpRoot, { recursive: true, force: true });
  } catch {
    // nothing to clean up
  }

  assertNoUnclassifiedRoutes(path.join(resultsDir, "server.log"));
}

/**
 * Wait for a process to disappear. `globalSetup` truncates the log, so it holds
 * only this run — but the last writes still have to land before it is read.
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

function assertNoUnclassifiedRoutes(logPath: string): void {
  let log: string;
  try {
    log = fs.readFileSync(logPath, "utf-8");
  } catch {
    return; // no log to audit
  }

  const hits = log
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.includes(UNCLASSIFIED_ROUTE_WARNING));

  if (hits.length > 0) {
    throw new Error(
      `The server reached ${hits.length} route(s) that are missing from the capability table ` +
        `(server/src/domain/capability.rs).\n` +
        `Every route a credential can reach must be classified, because an unclassified route ` +
        `answers 403 to everyone:\n  ${hits.join("\n  ")}`,
    );
  }
}
