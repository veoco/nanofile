import { test } from "node:test";
import assert from "node:assert/strict";
import { formatFileSize, formatBitrate } from "./format.js";

test("formatFileSize formats B/KB/MB/GB with 1000-based units", () => {
  assert.equal(formatFileSize(0), "0 B");
  assert.equal(formatFileSize(999), "999 B");
  assert.equal(formatFileSize(1000), "1.0 KB");
  assert.equal(formatFileSize(1500), "1.5 KB");
  assert.equal(formatFileSize(1000 * 1000), "1.0 MB");
  assert.equal(formatFileSize(1000 * 1000 * 1000), "1.0 GB");
  assert.equal(formatFileSize(2 * 1000 * 1000 * 1000), "2.0 GB");
});

test("formatFileSize returns empty for non-number input", () => {
  assert.equal(formatFileSize("1000"), "");
  assert.equal(formatFileSize(null), "");
  assert.equal(formatFileSize(undefined), "");
});

test("formatBitrate formats B/s/KB/s/MB/s", () => {
  assert.equal(formatBitrate(999), "999 B/s");
  assert.equal(formatBitrate(1000), "1.0 KB/s");
  assert.equal(formatBitrate(1000 * 1000), "1.0 MB/s");
});

test("formatBitrate returns empty for zero/negative/non-numeric", () => {
  assert.equal(formatBitrate(0), "");
  assert.equal(formatBitrate(-1), "");
  assert.equal(formatBitrate(null), "");
});
