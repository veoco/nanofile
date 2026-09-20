// local-time — the one place the Web UI turns a raw Unix timestamp into text.
//
// Templates carry nothing but the number and a mode:
//
//   <span data-ts="1756902896" data-ts-mode="relative"></span>
//
// `data-ts-mode` is `absolute` (the default: `YYYY-MM-DD HH:MM`), `relative`
// ("3 days ago") or `short` (the grid tile's "Sep 19"), and every mode puts the
// exact local stamp in the element's `title`. `data-ts-day` is the one other
// form: a marker on a row that the reader's local calendar day starts there, so
// a group header belongs above it. The server never pre-formats a time for a
// person to read; only machine-facing JSON payloads keep RFC3339.
import { __t } from "./i18n.js";
import {
  formatLocalDate,
  formatLocalDateTime,
  formatLocalShortDate,
  relativeParts,
} from "./format.js";

// Elements whose text changes as it gets older: re-rendered on a timer so a
// page left open does not keep claiming "Just now".
var live = [];

function seconds(el) {
  var ts = parseInt(el.dataset.ts, 10);
  return isNaN(ts) ? null : ts;
}

function title(el, ts) {
  var text = formatLocalDateTime(ts);
  if (text) el.title = text;
}

function render(el) {
  var ts = seconds(el);
  if (ts === null) return;
  var mode = el.dataset.tsMode || "absolute";

  if (mode === "relative") {
    title(el, ts);
    setLive(el, renderRelative(el, ts));
    return;
  }

  if (mode === "short") {
    title(el, ts);
    var short = formatLocalShortDate(ts);
    if (short) el.textContent = short;
    return;
  }

  title(el, ts);
  var absolute = formatLocalDateTime(ts);
  if (absolute) el.textContent = absolute;
}

// Relative text, falling back to the local calendar date once the difference
// reaches two weeks — the same cutoff the server used to apply.
function renderRelative(el, ts) {
  var parts = relativeParts(ts, Math.floor(Date.now() / 1000));
  if (!parts) {
    var date = formatLocalDate(ts);
    if (date) el.textContent = date;
    return false;
  }
  var text = __t(parts.key, parts.args);
  if (text) el.textContent = text;
  return true;
}

function setLive(el, isLive) {
  var idx = live.indexOf(el);
  if (isLive && idx === -1) live.push(el);
  else if (!isLive && idx !== -1) live.splice(idx, 1);
}

// `data-ts-day`: the element marks a row that starts a new local calendar day,
// so give it a group header above it.
function renderDayBoundary(el) {
  var ts = parseInt(el.dataset.tsDay, 10);
  if (isNaN(ts)) return;
  var key = formatLocalDate(ts);
  if (!key) return;
  var header = el.previousElementSibling;
  var current = header && header.classList.contains("nf-sec") ? header.textContent.trim() : null;
  if (
    current === key ||
    current === __t("activity.today") ||
    current === __t("activity.yesterday")
  ) {
    return;
  }
  if (header && header.classList.contains("nf-sec")) header.remove();

  var now = Math.floor(Date.now() / 1000);
  var text =
    key === formatLocalDate(now)
      ? __t("activity.today")
      : key === formatLocalDate(now - 86400)
        ? __t("activity.yesterday")
        : key;

  var section = document.createElement("div");
  section.className = "nf-sec";
  var heading = document.createElement("h2");
  heading.textContent = text;
  section.appendChild(heading);
  var rule = document.createElement("span");
  rule.className = "rule";
  rule.setAttribute("aria-hidden", "true");
  section.appendChild(rule);
  el.parentNode.insertBefore(section, el);
}

var TICK_MS = 60_000;
var started = false;

export function initLocalTime() {
  renderAll(document);

  // The file list is refreshed/paginated via AJAX, which swaps in new DOM
  // containing fresh `[data-ts]` elements. Watch the document body (not the
  // list container, which list.js replaces wholesale via outerHTML) so those
  // get rendered too, without list.js needing to know about this module.
  if ("MutationObserver" in window && document.body) {
    var observer = new MutationObserver(function (mutations) {
      mutations.forEach(function (m) {
        m.addedNodes.forEach(function (node) {
          if (node.nodeType !== 1) return;
          renderAll(node);
        });
      });
    });
    observer.observe(document.body, { childList: true, subtree: true });
  }

  // One timer for the whole page; when no cell is relative the list stays empty
  // and a tick costs nothing. Hidden tabs skip the pass: the text is relative to
  // now anyway, so the next visible tick catches up.
  if (!started) {
    started = true;
    setInterval(function () {
      if (document.visibilityState === "hidden") return;
      // Prune first: an AJAX list refresh replaces its rows wholesale, and a
      // detached node has nothing left to update.
      live = live.filter(function (el) {
        return el.isConnected;
      });
      live.forEach(function (el) {
        var ts = seconds(el);
        if (ts === null) return;
        setLive(el, renderRelative(el, ts));
      });
    }, TICK_MS);
  }
}

// A node from the observer, or the document on first paint.
export function renderAll(root) {
  if (root.matches && root.matches("[data-ts-day]")) renderDayBoundary(root);
  if (root.matches && root.matches("[data-ts]")) render(root);
  var days = root.querySelectorAll && root.querySelectorAll("[data-ts-day]");
  if (days) days.forEach(renderDayBoundary);
  var nested = root.querySelectorAll && root.querySelectorAll("[data-ts]");
  if (nested) nested.forEach(render);
}
