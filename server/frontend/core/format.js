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
