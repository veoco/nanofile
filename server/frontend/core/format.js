// format — pure number-to-string formatting helpers (no DOM, no globals).

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

// Format a Unix timestamp (seconds) as `YYYY-MM-DD HH:MM` in the browser's
// local timezone. One shape in every locale: a fixed-width stamp is what the
// tabular-nums columns were sized for, and it is what the tooltips show.
export function formatLocalDateTime(ts) {
  var d = new Date(ts * 1000);
  if (isNaN(d.getTime())) return "";
  var y = d.getFullYear();
  var m = ("0" + (d.getMonth() + 1)).slice(-2);
  var day = ("0" + d.getDate()).slice(-2);
  var h = ("0" + d.getHours()).slice(-2);
  var min = ("0" + d.getMinutes()).slice(-2);
  return y + "-" + m + "-" + day + " " + h + ":" + min;
}

// The document's language, which every `Intl` call below formats for.
function documentLocale() {
  return typeof document !== "undefined" && document.documentElement
    ? document.documentElement.lang
    : undefined;
}

// A compact month/day label for places where the full stamp does not fit (the
// grid tile's meta line): "Sep 19" in en, "9月19日" in zh. Intl picks the month
// name for the document's language, so this needs no per-locale month table.
export function formatLocalShortDate(ts) {
  var d = new Date(ts * 1000);
  if (isNaN(d.getTime())) return "";
  try {
    return new Intl.DateTimeFormat(documentLocale(), { month: "short", day: "numeric" }).format(d);
  } catch (ignored) {
    // Unsupported locale tag: fall back to the numeric form.
    return ("0" + (d.getMonth() + 1)).slice(-2) + "-" + ("0" + d.getDate()).slice(-2);
  }
}

// A local calendar date `YYYY-MM-DD`, used for timestamps too old for a
// relative form (>= 14 days) and for the activity page's day headers.
//
// Deliberately not `Intl`: this is the group key as well as a label, so it has
// to be locale-independent (and it is the form the server rendered before).
export function formatLocalDate(ts) {
  var d = new Date(ts * 1000);
  if (isNaN(d.getTime())) return "";
  var y = d.getFullYear();
  var m = ("0" + (d.getMonth() + 1)).slice(-2);
  var day = ("0" + d.getDate()).slice(-2);
  return y + "-" + m + "-" + day;
}

// Relative time matching the Android client's `translateCommitTime`:
// "Just now" → N seconds → N minutes → N hours → N days, then nothing (the
// caller falls back to `formatLocalDate` at >= 14 days).
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
