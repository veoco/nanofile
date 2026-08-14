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
