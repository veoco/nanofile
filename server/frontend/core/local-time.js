// local-time — the one place the Web UI turns a raw Unix timestamp into text.
//
// Templates carry nothing but the number and a mode:
//
//   <span data-ts="1756902896" data-ts-mode="relative"></span>
//
// `data-ts-mode` is `absolute` (the default: the reader's localized date and
// time) or `relative` ("3 days ago") or `short` (the grid tile's "Sep 19"), and
// every mode puts the exact local stamp in the element's `title`. `data-ts-day`
// is the one other form: a marker on a row that the reader's local calendar day
// starts there, so a group header belongs above it. The server never
// pre-formats a time for a person to read; only machine-facing JSON payloads
// keep RFC3339.
import { __t } from "./i18n.js";
import {
  formatLocalDate,
  formatLocalDateTime,
  formatLocalDateTimeLong,
  formatLocalMediumDate,
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
  var text = formatLocalDateTimeLong(ts);
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
    var date = formatLocalMediumDate(ts);
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

// The local calendar day a `data-ts-day` row belongs to, as a stable key.
function dayKey(el) {
  var ts = parseInt(el.dataset.tsDay, 10);
  if (isNaN(ts)) return null;
  return formatLocalDate(ts);
}

// The day header already sitting directly above `el`, if any.
function existingHeader(el) {
  var sib = el.previousElementSibling;
  return sib && sib.classList.contains("nf-sec") ? sib : null;
}

// The row above `el` in the feed, skipping any day header between them.
function previousRow(el) {
  var sib = el.previousElementSibling;
  while (sib && sib.classList.contains("nf-sec")) sib = sib.previousElementSibling;
  return sib && sib.hasAttribute("data-ts-day") ? sib : null;
}

// The header text for a day bucket. `key` is the ISO form the grouping compares
// and stays internal; everything a person reads is localized, so an older day
// gets the reader's own date rather than the raw key.
function dayLabel(key, ts) {
  var now = Math.floor(Date.now() / 1000);
  if (key === formatLocalDate(now)) return __t("activity.today");
  if (key === formatLocalDate(now - 86400)) return __t("activity.yesterday");
  return formatLocalMediumDate(ts);
}

// `data-ts-day`: the row opens a group when its local calendar day differs from
// the row above it. The day's header belongs above that first row only — every
// later row of the same day has another row directly above it, so comparing
// against the immediate sibling would label each row as its own group.
function renderDayBoundary(el) {
  var ts = parseInt(el.dataset.tsDay, 10);
  var key = dayKey(el);
  if (!key) return;

  var header = existingHeader(el);

  var prev = previousRow(el);
  if (prev && dayKey(prev) === key) {
    if (header) header.remove();
    return;
  }

  // A re-render of the same day must not stack a second header on its first row.
  if (header && header.dataset.dayKey === key) return;
  if (header) header.remove();

  var text = dayLabel(key, ts);

  var section = document.createElement("div");
  section.className = "nf-sec";
  section.dataset.dayKey = key;
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
