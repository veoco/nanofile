// common entry — loaded on every page. Bundles the core layer and all
// page-scoped modules; each module registers its own delegated event handlers
// (no window globals).
//
// i18n-init must be imported first: it publishes `window.__T` from the
// server-rendered JSON block, and ES module dependencies evaluate in import
// order, so every later module sees the dictionary.
import "../core/i18n-init.js";
import "../core/nav.js";
import "../core/pages.js";
import "../core/modal.js";
import "../core/search.js";
import "../pages/repos.js";
import "../pages/sysadmin.js";
import "../pages/starred.js";
import "../pages/trash.js";
import "../pages/shares.js";
import "../pages/api-keys.js";

// Render `[data-ts]` timestamps in the browser's local timezone.
import { initLocalTime } from "../core/local-time.js";
initLocalTime();

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
