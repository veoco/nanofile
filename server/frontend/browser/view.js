// view — view mode (list/grid/gallery), sort controls, and tag filtering.
// Dispatches "nanofile:viewchange" instead of calling into selection/pagination
// directly, so the module graph stays acyclic.
import { refreshFileList } from "./list.js";
import { getSort, getTagFilter, getVisibleView } from "./state.js";

export { getSort, getTagFilter, getVisibleView };

export function setMode(m) {
  var listView = document.querySelector(".js-file-list-view");
  var gridView = document.querySelector(".js-file-grid-view");
  var galleryView = document.querySelector(".js-gallery-view");
  var btnList = document.querySelector(".js-view-list");
  var btnGrid = document.querySelector(".js-view-grid");
  var btnGallery = document.querySelector(".js-view-gallery");
  var sortSection = document.querySelector(".js-sort-section");
  if (!listView || !gridView || !btnList || !btnGrid) return;

  // Hide sort buttons (Name/Modified/Size) in gallery mode
  if (sortSection) sortSection.classList.toggle("hidden", m === "gallery");

  // Reset all to hidden / inactive
  listView.classList.add("hidden");
  gridView.classList.add("hidden");
  if (galleryView) galleryView.classList.add("hidden");
  btnList.classList.remove("text-brand-500");
  btnList.classList.add("text-gray-400");
  btnGrid.classList.remove("text-brand-500");
  btnGrid.classList.add("text-gray-400");
  if (btnGallery) {
    btnGallery.classList.remove("text-brand-500");
    btnGallery.classList.add("text-gray-400");
  }

  if (m === "grid") {
    gridView.classList.remove("hidden");
    btnGrid.classList.remove("text-gray-400");
    btnGrid.classList.add("text-brand-500");
  } else if (m === "gallery") {
    if (galleryView) galleryView.classList.remove("hidden");
    if (btnGallery) {
      btnGallery.classList.remove("text-gray-400");
      btnGallery.classList.add("text-brand-500");
    }
  } else {
    listView.classList.remove("hidden");
    btnList.classList.remove("text-gray-400");
    btnList.classList.add("text-brand-500");
  }
  localStorage.setItem("fileViewMode", m);
  document.documentElement.dataset.view = m;
  document.dispatchEvent(new CustomEvent("nanofile:viewchange"));
}

// All three views are pre-rendered server-side, so switching is a pure
// client-side show/hide with no network round-trip.
function switchTo(m) {
  setMode(m);
  var main = document.querySelector("main");
  if (main) main.scrollTop = 0;
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
      if (upArrow) upArrow.style.fill = isActive && order === "asc" ? "var(--color-brand-500)" : "var(--color-gray-400)";
      if (downArrow) downArrow.style.fill = isActive && order === "desc" ? "var(--color-brand-500)" : "var(--color-gray-400)";
      btns[i].classList.toggle("text-brand-500", isActive);
      btns[i].classList.toggle("text-gray-400", !isActive);
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
  var order = field === s.sort ? (s.sort_order === "asc" ? "desc" : "asc") : "asc";
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
  sb.dataset.tagFilter = current === name ? "" : name;
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
