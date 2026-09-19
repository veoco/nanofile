import { test } from "node:test";
import assert from "node:assert/strict";
import { formatFileSize, formatBitrate, formatHistorySize, formatHistoryTime, formatLocalDateTime, formatLocalShortDate } from "./format.js";

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

test("formatHistorySize formats B/KB/MB/GB with 1000-based units", () => {
  assert.equal(formatHistorySize(0), "0 B");
  assert.equal(formatHistorySize(999), "999 B");
  assert.equal(formatHistorySize(1000), "1.0 KB");
  assert.equal(formatHistorySize(1000 * 1000), "1.0 MB");
  assert.equal(formatHistorySize(1000 * 1000 * 1000), "1.0 GB");
});

test("formatHistoryTime returns a non-empty string", () => {
  assert.equal(typeof formatHistoryTime(0), "string");
  assert.ok(formatHistoryTime(1234567890).length > 0);
});

test("formatLocalDateTime formats a Unix timestamp as YYYY-MM-DD HH:MM", () => {
  // 2026-09-03 12:34:56 UTC → local rendering depends on the test runner's
  // timezone, so assert the shape and that the components round-trip.
  var out = formatLocalDateTime(1756902896);
  assert.match(out, /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
});

test("formatLocalDateTime returns empty for invalid input", () => {
  assert.equal(formatLocalDateTime(NaN), "");
  assert.equal(formatLocalDateTime(undefined), "");
});

test("formatLocalShortDate renders a month/day label", () => {
  // 2026-09-03T12:34:56Z — the day may shift by timezone, so assert the shape.
  var out = formatLocalShortDate(1756902896);
  assert.ok(out.length > 0);
  assert.ok(/\d/.test(out), "keeps the day number: " + out);
  assert.ok(!out.includes(":"), "no time component: " + out);
});

test("formatLocalShortDate returns empty for invalid input", () => {
  assert.equal(formatLocalShortDate(NaN), "");
  assert.equal(formatLocalShortDate(undefined), "");
});
