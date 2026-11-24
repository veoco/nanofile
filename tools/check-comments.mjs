#!/usr/bin/env node
// Comment hygiene: reject text that cannot render.
//
// An encoding round-trip has already stored em dashes and arrows as mojibake in
// two separate change sets, and a module document written with `///` above the
// first `use` is attached to the import and never rendered by rustdoc. Neither
// is visible to rustc or to the tests, so both are checked here.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const MOJIBAKE = ["\u9225", "\u92ab", "\uFFFD"];
const CHECKED = /\.(rs|js|mjs|ts|toml|md)$/;

const files = execFileSync("git", ["ls-files", "-z"], { encoding: "utf8" })
  .split("\0")
  .filter(Boolean);

const errors = [];

for (const file of files) {
  if (!CHECKED.test(file)) continue;
  let text;
  try {
    text = readFileSync(file, "utf8");
  } catch {
    continue;
  }
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    for (const bad of MOJIBAKE) {
      if (lines[i].includes(bad)) {
        errors.push(
          `${file}:${i + 1}: corrupted comment text (${bad}); restore the em dash/arrow`,
        );
      }
    }
  }
  if (file.endsWith(".rs")) checkModuleDoc(file, lines);
}

function checkModuleDoc(file, lines) {
  let i = 0;
  while (i < lines.length) {
    const trimmed = lines[i].trim();
    if (!trimmed || trimmed.startsWith("#![")) {
      i++;
      continue;
    }
    break;
  }
  if (i >= lines.length) return;
  const first = lines[i].trimStart();
  if (!first.startsWith("///") || first.startsWith("////")) return;

  let j = i;
  while (j < lines.length && lines[j].trimStart().startsWith("///")) j++;

  // Skip attributes and ordinary comments between the doc group and the item.
  let k = j;
  while (k < lines.length) {
    const trimmed = lines[k].trim();
    if (!trimmed || trimmed.startsWith("#[") || trimmed.startsWith("//")) {
      k++;
      continue;
    }
    break;
  }
  if (k >= lines.length) return;

  if (/^(pub(\s*\([^)]*\))?\s+)?(use|mod)\b/.test(lines[k].trim())) {
    errors.push(
      `${file}:${i + 1}: module doc uses /// above an import and never renders; use //!`,
    );
  }
}

if (errors.length) {
  for (const error of errors) console.error(error);
  console.error(`\n${errors.length} comment defect(s); see docs/dev/contributing.md.`);
  process.exit(1);
}
