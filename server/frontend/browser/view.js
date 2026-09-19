// view — view mode (list/grid/gallery), sort controls, and tag filtering.
// Dispatches "nanofile:viewchange" instead of calling into selection/pagination
// directly, so the module graph stays acyclic.
import { refreshFileList } from "./list.js";
import { getSort, getTagFilter, getVisibleView } from "./state.js";
import { nextSortOrder, nextTagFilter } from "./view-logic.js";

export { getSort, getTagFilter, getVisibleView };

export function setMode(m) {
  var listView = document.querySelector(".js-file-list-view");
  var gridView = document.querySelector(".js-file-grid-view");
  var galleryView = document.querySelector(".js-gallery-view");
  var sortSection = document.querySelector(".js-sort-section");
  if (!listView || !gridView) return;

  // Hide sort buttons (Name/Modified/Size) in gallery mode
  if (sortSection) sortSection.classList.toggle("hidden", m === "gallery");

  // Reset all to hidden; which view is shown is decided here, which view is
  // *active* in the switcher is decided by `html[data-view]` in CSS — one
  // source of truth that also survives the AJAX refresh replacing the toolbar.
  listView.classList.add("hidden");
  gridView.classList.add("hidden");
  if (galleryView) galleryView.classList.add("hidden");

  if (m === "grid") {
    gridView.classList.remove("hidden");
  } else if (m === "gallery") {
    if (galleryView) galleryView.classList.remove("hidden");
  } else {
    listView.classList.remove("hidden");
  }
  localStorage.setItem("fileViewMode", m);
  document.documentElement.dataset.view = m;
  document.dispatchEvent(new CustomEvent("nanofile:viewchange"));
}

// All three views are pre-rendered server-side, so switching is a pure
// client-side show/hide with no network round-trip.
function switchTo(m) {
  setMode(m);
  var scroller = document.getElementById("nf-list-scroll");
  if (scroller) scroller.scrollTop = 0;
}

// Event delegation on document so view toggle works after partial refresh
document.addEventListener("click", function (e) {
  var btn = e.target.closest(".js-view-list");
  if (btn) { switchTo("list"); return; }
  btn = e.target.closest(".js-view-grid");
  if (btn) { switchTo("grid"); return; }
  btn = e.target.closest(".js-view-gallery");
  if (btn) { switchTo("gallery"); }
});

// Initialize mode from localStorage on page load
var mode = localStorage.getItem("fileViewMode") || "list";
setMode(mode);

// ─── Sort controls ──────────────────────────────────────────────────────
function applySortUI(field, order) {
  var sortBar = document.querySelector(".js-sort-bar");
  if (sortBar) {
    sortBar.dataset.sortField = field;
    sortBar.dataset.sortOrder = order;
    var btns = sortBar.querySelectorAll(".js-sort-btn");
    for (var i = 0; i < btns.length; i++) {
      var f = btns[i].dataset.sort;
      var isActive = f === field;
      var upArrow = btns[i].querySelector(".js-sort-arrow-up");
      var downArrow = btns[i].querySelector(".js-sort-arrow-down");
      if (upArrow) upArrow.style.fill = isActive && order === "asc" ? "var(--color-accent)" : "var(--color-ink-3)";
      if (downArrow) downArrow.style.fill = isActive && order === "desc" ? "var(--color-accent)" : "var(--color-ink-3)";
      btns[i].classList.toggle("on", isActive);
    }
  }
}

export function initSortUI() {
  var sortBar = document.querySelector(".js-sort-bar");
  if (!sortBar) return;
  applySortUI(sortBar.dataset.sortField || "name", sortBar.dataset.sortOrder || "asc");
}

function setSort(field) {
  var s = getSort();
  var order = nextSortOrder(field, s.sort, s.sort_order);
  localStorage.setItem("fileSortField", field);
  localStorage.setItem("fileSortOrder", order);
  applySortUI(field, order);
  refreshFileList();
}

document.addEventListener("click", function (e) {
  var btn = e.target.closest(".js-sort-btn");
  if (btn) { setSort(btn.dataset.sort); return; }
});

// ─── Tag filter ─────────────────────────────────────────────────────────
function applyTagFilter(name) {
  var sb = document.querySelector(".js-sort-bar");
  if (!sb) return;
  var current = sb.dataset.tagFilter || "";
  sb.dataset.tagFilter = nextTagFilter(current, name);
  refreshFileList();
}

document.addEventListener("click", function (e) {
  var btn = e.target.closest(".js-tag-filter-btn");
  if (btn) { e.stopPropagation(); applyTagFilter(btn.dataset.tag); return; }
  var entryTag = e.target.closest(".js-entry-tag");
  if (entryTag) { e.stopPropagation(); applyTagFilter(entryTag.dataset.tag); }
});

// Initialize sort UI from server-rendered data attributes
initSortUI();

// After a partial list refresh, re-apply the sort UI (server replaced the
// sort-bar DOM) and restore the current view mode.
document.addEventListener("nanofile:list-refreshed", function () {
  initSortUI();
  setMode(localStorage.getItem("fileViewMode") || "list");
});
