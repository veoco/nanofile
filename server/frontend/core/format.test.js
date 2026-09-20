import { test } from "node:test";
import assert from "node:assert/strict";
import { formatFileSize, formatBitrate, formatHistorySize, formatLocalDateTime, formatLocalDateTimeLong, formatLocalMediumDate, formatLocalShortDate, formatLocalDate, relativeParts } from "./format.js";

// The time helpers format for `document.documentElement.lang`. Pin it so the
// assertions follow the language under test and not the machine's default
// locale (and its default timezone, which is why these compare against the same
// `Intl` call rather than a literal).
function useLang(lang) {
  globalThis.document = { documentElement: { lang } };
}

function intl(lang, options, ts) {
  return new Intl.DateTimeFormat(lang, options).format(new Date(ts * 1000));
}

const SHORT = { dateStyle: "short", timeStyle: "short" };
const MEDIUM = { dateStyle: "medium", timeStyle: "medium" };
const DATE = { dateStyle: "medium" };
const MONTH_DAY = { month: "short", day: "numeric" };

// 2026-09-03T12:34:56Z.
const TS = 1756902896;

useLang("en");

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

test("formatLocalDateTime renders the reader's short date and time", () => {
  useLang("en");
  assert.equal(formatLocalDateTime(TS), intl("en", SHORT, TS));
  useLang("zh");
  assert.equal(formatLocalDateTime(TS), intl("zh", SHORT, TS));
});

test("formatLocalDateTime follows the document language's clock", () => {
  // en is a 12-hour clock, zh a 24-hour one: the two must not render alike.
  useLang("en");
  var en = formatLocalDateTime(TS);
  useLang("zh");
  var zh = formatLocalDateTime(TS);
  assert.notEqual(en, zh);
  assert.match(en, /(AM|PM)/);
  assert.doesNotMatch(zh, /(AM|PM)/);
});

test("formatLocalDateTime returns empty for invalid input", () => {
  useLang("en");
  assert.equal(formatLocalDateTime(NaN), "");
  assert.equal(formatLocalDateTime(undefined), "");
  assert.equal(formatLocalDateTimeLong(NaN), "");
  assert.equal(formatLocalMediumDate(undefined), "");
});

test("formatLocalDateTimeLong spells the stamp out with seconds", () => {
  useLang("en");
  var out = formatLocalDateTimeLong(TS);
  assert.equal(out, intl("en", MEDIUM, TS));
  assert.match(out, /\d{1,2}:\d{2}:\d{2}/, "keeps the seconds: " + out);
});

test("formatLocalMediumDate renders a date with no time", () => {
  useLang("en");
  var out = formatLocalMediumDate(TS);
  assert.equal(out, intl("en", DATE, TS));
  assert.ok(!out.includes(":"), "no time component: " + out);
  useLang("zh");
  assert.equal(formatLocalMediumDate(TS), intl("zh", DATE, TS));
});

test("a locale tag Intl rejects falls back to the numeric form", () => {
  useLang("!!");
  assert.match(formatLocalDateTime(TS), /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  assert.match(formatLocalDateTimeLong(TS), /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  assert.match(formatLocalMediumDate(TS), /^\d{4}-\d{2}-\d{2}$/);
  assert.match(formatLocalShortDate(TS), /^\d{2}-\d{2}$/);
});

test("formatLocalShortDate renders a month/day label", () => {
  useLang("en");
  var out = formatLocalShortDate(TS);
  assert.equal(out, intl("en", MONTH_DAY, TS));
  assert.ok(!out.includes(":"), "no time component: " + out);
});

test("formatLocalShortDate returns empty for invalid input", () => {
  useLang("en");
  assert.equal(formatLocalShortDate(NaN), "");
  assert.equal(formatLocalShortDate(undefined), "");
});

test("formatLocalDate stays a locale-independent key", () => {
  // The activity feed buckets rows by this string, so it must not follow the
  // document language: a label for a person comes from formatLocalMediumDate.
  useLang("zh");
  assert.match(formatLocalDate(TS), /^\d{4}-\d{2}-\d{2}$/);
  var d = new Date(TS * 1000);
  var expected =
    d.getFullYear() +
    "-" +
    String(d.getMonth() + 1).padStart(2, "0") +
    "-" +
    String(d.getDate()).padStart(2, "0");
  assert.equal(formatLocalDate(TS), expected);
});

test("formatLocalDate returns empty for invalid input", () => {
  assert.equal(formatLocalDate(NaN), "");
  assert.equal(formatLocalDate(undefined), "");
});

test("relativeParts picks the singular key for a count of one", () => {
  var now = 1_800_000_000;
  assert.deepEqual(relativeParts(now, now), { key: "activity.just_now", args: null });
  assert.deepEqual(relativeParts(now - 60, now), { key: "activity.minute_ago", args: null });
  assert.deepEqual(relativeParts(now - 3600, now), { key: "activity.hour_ago", args: null });
  assert.deepEqual(relativeParts(now - 86400, now), { key: "activity.day_ago", args: null });
});

test("relativeParts passes the count for every plural bucket", () => {
  var now = 1_800_000_000;
  assert.deepEqual(relativeParts(now - 2, now), { key: "activity.seconds_ago", args: { n: 2 } });
  assert.deepEqual(relativeParts(now - 5 * 60, now), { key: "activity.minutes_ago", args: { n: 5 } });
  assert.deepEqual(relativeParts(now - 5 * 3600, now), { key: "activity.hours_ago", args: { n: 5 } });
  assert.deepEqual(relativeParts(now - 3 * 86400, now), { key: "activity.days_ago", args: { n: 3 } });
});

test("relativeParts treats a future timestamp as just now", () => {
  var now = 1_800_000_000;
  assert.deepEqual(relativeParts(now + 3600, now), { key: "activity.just_now", args: null });
});

test("relativeParts gives up at fourteen days", () => {
  var now = 1_800_000_000;
  assert.equal(relativeParts(now - 13 * 86400, now).key, "activity.days_ago");
  assert.equal(relativeParts(now - 14 * 86400, now), null);
});
