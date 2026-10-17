// local-time — the one place the Web UI turns a raw Unix timestamp into text. Templates carry
// only the number and a mode (`absolute`, `relative`, `short`), and every mode puts the exact
// local stamp in `title`; `data-ts-day` marks the first row of a local calendar day so a group
// header belongs above it. The server never pre-formats a time for a person; only machine JSON
// keeps RFC3339.
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

// Relative text, falling back to the local date once the difference reaches two weeks.
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

// The header text for a day bucket: `key` stays internal, and everything read is localized.
function dayLabel(key, ts) {
  var now = Math.floor(Date.now() / 1000);
  if (key === formatLocalDate(now)) return __t("activity.today");
  if (key === formatLocalDate(now - 86400)) return __t("activity.yesterday");
  return formatLocalMediumDate(ts);
}

// `data-ts-day`: the row opens a group when its local day differs from the row above. Comparing
// against the immediate sibling would label every row its own group, so the header goes above the
// first row only.
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
  el.parentNode.insertBefore(section, el);
}

var TICK_MS = 60_000;
var started = false;

// The feed ships ungrouped and CSS holds it out of the first paint until this pass runs: a band
// is 41px of layout, so painting before the bands exist moves every row below them.
function markFeedsGrouped() {
  document.querySelectorAll("[data-day-group]").forEach(function (el) {
    el.dataset.dayGrouped = "1";
  });
}

export function initLocalTime() {
  // `finally`: a render that throws must not leave the feed held back forever.
  try {
    renderAll(document);
  } finally {
    markFeedsGrouped();
  }

// The file list is refreshed via AJAX, which swaps in new `[data-ts]` nodes. Observe the body
// (not the container, which list.js replaces via outerHTML) so list.js need not know this module.
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

// One timer for the page; an empty list costs nothing and hidden tabs skip the pass.
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
