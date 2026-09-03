// local-time — render `[data-ts]` elements (Unix seconds) in the browser's
// local timezone. The server embeds raw timestamps; this module fills the
// visible text so users in any timezone see their local time.
import { formatLocalDateTime } from "./format.js";

function render(el) {
  var ts = parseInt(el.dataset.ts, 10);
  if (isNaN(ts)) return;
  var text = formatLocalDateTime(ts);
  if (text) el.textContent = text;
}

function renderTitle(el) {
  var ts = parseInt(el.dataset.tsTitle, 10);
  if (isNaN(ts)) return;
  var text = formatLocalDateTime(ts);
  if (text) el.title = text;
}

export function initLocalTime() {
  document.querySelectorAll("[data-ts]").forEach(render);
  document.querySelectorAll("[data-ts-title]").forEach(renderTitle);

  // The file list is refreshed/paginated via AJAX, which swaps in new DOM
  // containing fresh `[data-ts]` elements. Watch the list container so those
  // get rendered too, without list.js needing to know about this module.
  var container = document.querySelector(".file-list-container");
  if (container && "MutationObserver" in window) {
    var observer = new MutationObserver(function (mutations) {
      mutations.forEach(function (m) {
        m.addedNodes.forEach(function (node) {
          if (node.nodeType !== 1) return;
          if (node.matches && node.matches("[data-ts]")) render(node);
          if (node.matches && node.matches("[data-ts-title]")) renderTitle(node);
          var nested = node.querySelectorAll && node.querySelectorAll("[data-ts], [data-ts-title]");
          if (nested) nested.forEach(function (n) {
            if (n.hasAttribute("data-ts")) render(n);
            if (n.hasAttribute("data-ts-title")) renderTitle(n);
          });
        });
      });
    });
    observer.observe(container, { childList: true, subtree: true });
  }
}
