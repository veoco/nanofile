// local-time — render `[data-ts]` elements (Unix seconds) in the browser's
// local timezone. The server embeds raw timestamps; this module fills the
// visible text so users in any timezone see their local time.
import { formatLocalDateTime, formatLocalShortDate } from "./format.js";

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

// `data-ts-short` = a compact month/day label for the grid tile's meta line
// ("Sep 19", "9月19日"). Intl handles the month name in the document's locale,
// which is what the server can't do without shipping a month table per language.
function renderShort(el) {
  var ts = parseInt(el.dataset.tsShort, 10);
  if (isNaN(ts)) return;
  var text = formatLocalShortDate(ts);
  if (text) el.textContent = text;
}

export function initLocalTime() {
  document.querySelectorAll("[data-ts]").forEach(render);
  document.querySelectorAll("[data-ts-title]").forEach(renderTitle);
  document.querySelectorAll("[data-ts-short]").forEach(renderShort);

  // The file list is refreshed/paginated via AJAX, which swaps in new DOM
  // containing fresh `[data-ts]` elements. Watch the document body (not the
  // list container, which list.js replaces wholesale via outerHTML) so those
  // get rendered too, without list.js needing to know about this module.
  if ("MutationObserver" in window) {
    var observer = new MutationObserver(function (mutations) {
      mutations.forEach(function (m) {
        m.addedNodes.forEach(function (node) {
          if (node.nodeType !== 1) return;
          if (node.matches && node.matches("[data-ts]")) render(node);
          if (node.matches && node.matches("[data-ts-title]")) renderTitle(node);
          if (node.matches && node.matches("[data-ts-short]")) renderShort(node);
          var nested = node.querySelectorAll && node.querySelectorAll("[data-ts], [data-ts-title], [data-ts-short]");
          if (nested) nested.forEach(function (n) {
            if (n.hasAttribute("data-ts")) render(n);
            if (n.hasAttribute("data-ts-title")) renderTitle(n);
            if (n.hasAttribute("data-ts-short")) renderShort(n);
          });
        });
      });
    });
    observer.observe(document.body, { childList: true, subtree: true });
  }
}
