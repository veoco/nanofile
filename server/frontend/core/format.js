// format — number and timestamp-to-string helpers.
//
// The size/bitrate helpers are pure. The time helpers are not: they read
// `document.documentElement.lang` and ask `Intl` for the reader's own
// conventions — date order, separators, month names, 12- vs 24-hour clock — so
// nothing here hand-assembles a stamp except the numeric fallback used when
// `Intl` rejects the locale tag.

export function formatBitrate(bytesPerSec) {
  if (!bytesPerSec || bytesPerSec <= 0) return '';
  if (bytesPerSec >= 1000 * 1000) return (bytesPerSec / (1000 * 1000)).toFixed(1) + ' MB/s';
  if (bytesPerSec >= 1000) return (bytesPerSec / 1000).toFixed(1) + ' KB/s';
  return Math.round(bytesPerSec) + ' B/s';
}

export function formatFileSize(size) {
  if (typeof size !== 'number') return '';
  if (size >= 1000 * 1000 * 1000) return (size / (1000 * 1000 * 1000)).toFixed(1) + ' GB';
  if (size >= 1000 * 1000) return (size / (1000 * 1000)).toFixed(1) + ' MB';
  if (size >= 1000) return (size / 1000).toFixed(1) + ' KB';
  return size + ' B';
}

// History dialog size — decimal (1000-based) units, matching formatFileSize.
export function formatHistorySize(n) {
  if (n >= 1000 * 1000 * 1000) return (n / (1000 * 1000 * 1000)).toFixed(1) + ' GB';
  if (n >= 1000 * 1000) return (n / (1000 * 1000)).toFixed(1) + ' MB';
  if (n >= 1000) return (n / 1000).toFixed(1) + ' KB';
  return n + ' B';
}

// The document's language, which every `Intl` call below formats for.
function documentLocale() {
  return typeof document !== "undefined" && document.documentElement
    ? document.documentElement.lang
    : undefined;
}

// `Intl.DateTimeFormat` construction is not free and a page renders one cell per
// file, so each option set is built once per locale and kept. A locale tag `Intl`
// rejects is cached as `null`; callers then take the numeric fallback without
// throwing per cell.
var formatters = {};

function intl(options, key) {
  var locale = documentLocale();
  var cacheKey = (locale || "") + "|" + key;
  if (cacheKey in formatters) return formatters[cacheKey];
  var made = null;
  try {
    made = new Intl.DateTimeFormat(locale || undefined, options);
  } catch (ignored) {
    made = null;
  }
  formatters[cacheKey] = made;
  return made;
}

function pad(n) {
  return ("0" + n).slice(-2);
}

function numericDate(d) {
  return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate());
}

function numericDateTime(d) {
  return numericDate(d) + " " + pad(d.getHours()) + ":" + pad(d.getMinutes());
}

// `Intl` when it can format for the document's language, `fallback(d)` when it
// cannot. An unparseable timestamp is empty either way, never "Invalid Date".
function localized(ts, options, key, fallback) {
  var d = new Date(ts * 1000);
  if (isNaN(d.getTime())) return "";
  var fmt = intl(options, key);
  return fmt ? fmt.format(d) : fallback(d);
}

// The display form for a `data-ts` cell: the reader's own short date and short
// time — "9/19/25, 3:04 PM" in en, "2025/9/19 15:04" in zh. The width therefore
// varies with the language and the clock, which is why no column reserves one.
export function formatLocalDateTime(ts) {
  return localized(ts, { dateStyle: "short", timeStyle: "short" }, "short", numericDateTime);
}

// The `title` form: the same instant spelled out with seconds, so hovering a
// relative cell ("3 days ago") still gives an exact answer.
export function formatLocalDateTimeLong(ts) {
  return localized(ts, { dateStyle: "medium", timeStyle: "medium" }, "medium", numericDateTime);
}

// A date with no time: the activity page's day headers, and the relative form's
// fallback once a stamp is too old for "N days ago".
export function formatLocalMediumDate(ts) {
  return localized(ts, { dateStyle: "medium" }, "date", numericDate);
}

// A compact month/day label for places where the full stamp does not fit (the
// grid tile's meta line): "Sep 19" in en, "9月19日" in zh. Intl picks the month
// name for the document's language, so this needs no per-locale month table.
export function formatLocalShortDate(ts) {
  return localized(ts, { month: "short", day: "numeric" }, "monthday", function (d) {
    return pad(d.getMonth() + 1) + "-" + pad(d.getDate());
  });
}

// A local calendar date `YYYY-MM-DD`. Deliberately not `Intl`: this is the
// group key the activity feed compares and buckets by, so it has to be
// locale-independent. Labels for a person come from `formatLocalMediumDate`.
export function formatLocalDate(ts) {
  var d = new Date(ts * 1000);
  if (isNaN(d.getTime())) return "";
  return numericDate(d);
}

// Relative time matching the Android client's `translateCommitTime`:
// "Just now" → N seconds → N minutes → N hours → N days, then nothing (the
// caller falls back to `formatLocalMediumDate` at >= 14 days).
//
// Returns `{ key, args }` for the i18n table rather than a formatted string, so
// this stays a pure function with no `window.__T` dependency. A count of 1 gets
// its own key, so nothing reads "1 days ago".
export function relativeParts(ts, nowSeconds) {
  var diff = nowSeconds - ts;
  if (diff <= 0) return { key: "activity.just_now", args: null };
  var days = Math.floor(diff / 86400);
  if (days >= 14) return null;
  if (days > 0) return count(days, "activity.day_ago", "activity.days_ago");
  var hours = Math.floor(diff / 3600);
  if (hours > 0) return count(hours, "activity.hour_ago", "activity.hours_ago");
  var minutes = Math.floor(diff / 60);
  if (minutes > 0) return count(minutes, "activity.minute_ago", "activity.minutes_ago");
  return count(diff, "activity.second_ago", "activity.seconds_ago");
}

function count(n, one, many) {
  if (n === 1) return { key: one, args: null };
  return { key: many, args: { n: n } };
}
