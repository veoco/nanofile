// toast — toast notifications + global loading bar.
import { escapeHtml } from "./utils.js";

var toastContainer = null;
function initToast() {
  toastContainer = document.createElement("div");
  toastContainer.className =
    "fixed top-4 right-4 z-[9999] flex flex-col gap-2 pointer-events-none";
  toastContainer.setAttribute("aria-live", "polite");
  toastContainer.setAttribute("aria-relevant", "additions removals");
  document.body.appendChild(toastContainer);
}

export function showToast(message, type, duration) {
  type = type || "success";
  duration = duration || 4000;
  if (!toastContainer) initToast();

  var colors = {
    success:
      "bg-green-50 border-green-200 text-green-800",
    error:
      "bg-red-50 border-red-200 text-red-800",
    info:
      "bg-brand-50 border-brand-200 text-brand-800",
  };

  var icons = {
    success:
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/>',
    error:
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-2.5L13.732 4c-.77-.833-1.964-.833-2.732 0L4.082 16.5c-.77.833.192 2.5 1.732 2.5z"/>',
    info:
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/>',
  };

  var el = document.createElement("div");
  el.className =
    "pointer-events-auto flex items-center gap-3 rounded-xl border px-4 py-3 shadow-lg animate-slide-in " +
    (colors[type] || colors.success);
  el.innerHTML =
    '<svg class="h-5 w-5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">' +
    (icons[type] || icons.success) +
    '</svg><p class="text-sm font-medium flex-1">' +
    escapeHtml(message) +
    '</p><button class="flex-shrink-0 rounded-md p-1 opacity-60 hover:opacity-100 transition-opacity" onclick="this.parentElement.classList.add(\'animate-slide-out\');setTimeout(function(){this.parentElement.remove()}.bind(this),250)">' +
    '<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>' +
    "</button>";

  toastContainer.appendChild(el);

  setTimeout(function () {
    el.classList.add("animate-slide-out");
    setTimeout(function () { if (el.parentNode) el.remove(); }, 250);
  }, duration);
}

export const Toast = {
  show: showToast,
  success: function (m) { showToast(m, "success"); },
  error: function (m) { showToast(m, "error"); },
  info: function (m) { showToast(m, "info"); },
};

// ─── Loading bar ────────────────────────────────────────────────────────
var loadingBar = document.getElementById("loading-bar");
export function showLoading() {
  if (loadingBar) loadingBar.classList.remove("hidden");
}
export function hideLoading() {
  if (loadingBar) loadingBar.classList.add("hidden");
}
