// common entry — loaded on every page. Bundles the core layer and re-exposes
// the small surface that inline template <script> blocks still rely on.
import { __t } from "../core/i18n.js";
import { escapeHtml, escapeAttr, getCookie, encodeFilePath, unquote, safeColor } from "../core/utils.js";
import { apiFetch } from "../core/api.js";
import { Toast, showToast, showLoading, hideLoading } from "../core/toast.js";
import { ConfirmDialog } from "../core/confirm.js";
import "../core/nav.js";
import { showQuickCreate, hideQuickCreate, submitQuickCreate } from "../core/pages.js";

// Exposed for inline <script> blocks in templates.
window.__t = __t;
window.escapeHtml = escapeHtml;
window.escapeAttr = escapeAttr;
window.getCookie = getCookie;
window.encodeFilePath = encodeFilePath;
window.unquote = unquote;
window.safeColor = safeColor;
window.apiFetch = apiFetch;
window.Toast = Toast;
window.showToast = showToast;
window.showLoading = showLoading;
window.hideLoading = hideLoading;
window.ConfirmDialog = ConfirmDialog;
window.showQuickCreate = showQuickCreate;
window.hideQuickCreate = hideQuickCreate;
window.submitQuickCreate = submitQuickCreate;

// ─── Response time display ──────────────────────────────────────────────
var respTimeEl = document.getElementById("resp-time");
if (respTimeEl) {
  window.addEventListener("load", function () {
    var loadTime = performance.now();
    var display = loadTime >= 2000
      ? (loadTime / 1000).toFixed(1) + "s"
      : Math.round(loadTime) + "ms";
    respTimeEl.textContent = display;
    respTimeEl.classList.remove("opacity-0");
  });
}
