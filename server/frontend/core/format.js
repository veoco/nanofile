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

// History dialog size — 1024-based units (differs from formatFileSize's
// 1000-based units; do not merge).
export function formatHistorySize(n) {
  if (n >= 1024 * 1024 * 1024) return (n / (1024 * 1024 * 1024)).toFixed(1) + ' GB';
  if (n >= 1024 * 1024) return (n / (1024 * 1024)).toFixed(1) + ' MB';
  if (n >= 1024) return (n / 1024).toFixed(1) + ' KB';
  return n + ' B';
}

export function formatHistoryTime(ts) {
  var d = new Date(ts * 1000);
  return d.toLocaleString();
}

// Format a Unix timestamp (seconds) as `YYYY-MM-DD HH:MM` in the browser's
// local timezone. Matches the server-side `format_ts` shape but localizes it.
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
