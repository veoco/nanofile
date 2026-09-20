import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

/**
 * Move every file whose name starts with `prefix` to a fixed date.
 *
 * The gallery groups by month, so any spec that needs a month boundary — the
 * only thing that puts a sticky group header in the band the bottom drawer
 * covers, or slides one across the toolbar — needs files in two months. Nothing
 * in the API sets a dirent's mtime (an upload always stamps "now"), so this
 * edits the temp SQLite DB the run is using, where the mtime sits in the parent
 * directory's dirent JSON.
 *
 * Callers pass a prefix unique to their own repo, so one spec cannot disturb
 * another's files. Returns the number of dirents patched: assert it found the
 * files, or a rename would quietly turn the caller's month boundary into a
 * single group.
 */
export function backdateFileMtimes(prefix: string, seconds: number): number {
  const { tmpRoot } = JSON.parse(
    fs.readFileSync(path.join(process.cwd(), "test-results", ".server.json"), "utf-8"),
  ) as { tmpRoot: string };

  const db = new DatabaseSync(path.join(tmpRoot, "nanofile.db"));
  try {
    const rows = db.prepare("SELECT id, data FROM fs_objects").all() as {
      id: number;
      data: string;
    }[];
    let patched = 0;
    for (const row of rows) {
      if (!String(row.data).includes(`"name": "${prefix}`)) continue;
      const obj = JSON.parse(String(row.data));
      for (const dirent of obj.dirents ?? []) {
        if (typeof dirent.name === "string" && dirent.name.startsWith(prefix)) {
          dirent.mtime = seconds;
          patched += 1;
        }
      }
      db.prepare("UPDATE fs_objects SET data = ? WHERE id = ?").run(JSON.stringify(obj), row.id);
    }
    return patched;
  } finally {
    db.close();
  }
}
