// toast — toast notifications + global loading bar.
import { escapeHtml } from "./utils.js";
import { __t } from "./i18n.js";

var toastContainer = null;
function initToast() {
  toastContainer = document.createElement("div");
  toastContainer.className =
    "fixed top-4 right-4 z-top flex flex-col gap-2 pointer-events-none";
  toastContainer.setAttribute("aria-live", "polite");
  toastContainer.setAttribute("aria-relevant", "additions removals");
  document.body.appendChild(toastContainer);
}

export function showToast(message, type, duration) {
  type = type || "success";
  duration = duration || 4000;
  if (!toastContainer) initToast();

  // `.nf-toast` carries the layout and `.is-ok|is-err|is-warn` the state colour
  // — the same tint + border + text weight as `.nf-banner`. No variant means a
  // neutral informational toast.
  var variants = { success: "is-ok", error: "is-err", warn: "is-warn" };

  var icons = {
    success:
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/>',
    error:
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-2.5L13.732 4c-.77-.833-1.964-.833-2.732 0L4.082 16.5c-.77.833.192 2.5 1.732 2.5z"/>',
    warn:
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-2.5L13.732 4c-.77-.833-1.964-.833-2.732 0L4.082 16.5c-.77.833.192 2.5 1.732 2.5z"/>',
    info:
      '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/>',
  };

  var el = document.createElement("div");
  el.className = "nf-toast animate-slide-in " + (variants[type] || "");
  el.setAttribute("role", type === "error" ? "alert" : "status");
  el.innerHTML =
    '<svg fill="none" stroke="currentColor" viewBox="0 0 24 24">' +
    (icons[type] || icons.info) +
    '</svg><p class="flex-1 min-w-0">' +
    escapeHtml(message) +
    '</p><button type="button" class="nf-toast-close" aria-label="' +
    escapeHtml(__t("common.close")) +
    '">' +
    '<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>' +
    "</button>";

  toastContainer.appendChild(el);

  // One dismissal path for the close button and the timer, so a click cannot
  // leave the timer behind to touch a node that is already gone.
  var dismissed = false;
  function dismiss() {
    if (dismissed) return;
    dismissed = true;
    el.classList.add("animate-slide-out");
    setTimeout(function () { if (el.parentNode) el.remove(); }, 250);
  }
  el.querySelector(".nf-toast-close").addEventListener("click", dismiss);
  setTimeout(dismiss, duration);
}

export const Toast = {
  show: showToast,
  success: function (m) { showToast(m, "success"); },
  error: function (m) { showToast(m, "error"); },
  warn: function (m) { showToast(m, "warn"); },
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
