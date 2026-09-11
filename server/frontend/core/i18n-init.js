// i18n-init — publish the server-rendered translation table.
//
// The dictionary used to be an inline `window.__T = {...}` script, which forced
// script-src to allow 'unsafe-inline'. It now travels in a
// `<script type="application/json" id="__i18n">` data block (not executable, so
// CSP does not apply) and is published here.
//
// `common.js` imports this module first: ES module dependencies are evaluated
// in import order, so `window.__T` is in place before any other module runs.
(function () {
  if (window.__T) return;
  var el = document.getElementById("__i18n");
  if (!el) return;
  try {
    window.__T = JSON.parse(el.textContent || "{}");
  } catch (e) {
    window.__T = {};
  }
})();
