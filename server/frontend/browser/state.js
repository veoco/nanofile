// state — pure read accessors for the current view/sort/tag state. Split out so
// both view.js (writes) and list.js (reads while building URLs) can depend on it
// without forming a module cycle.

export function getSort() {
  var sortBar = document.querySelector(".js-sort-bar");
  if (sortBar) {
    return { sort: sortBar.dataset.sortField || "name", sort_order: sortBar.dataset.sortOrder || "asc" };
  }
  return { sort: localStorage.getItem("fileSortField") || "name", sort_order: localStorage.getItem("fileSortOrder") || "asc" };
}

export function getTagFilter() {
  var sb = document.querySelector(".js-sort-bar");
  return sb ? (sb.dataset.tagFilter || "") : "";
}

export function getVisibleView() {
  var gv = document.querySelector(".js-gallery-view");
  if (gv && !gv.classList.contains("hidden")) return "gallery";
  var gridV = document.querySelector(".js-file-grid-view");
  if (gridV && !gridV.classList.contains("hidden")) return "grid";
  return "list";
}

export function getVisibleViewContainer() {
  var listView = document.querySelector(".js-file-list-view");
  var gridView = document.querySelector(".js-file-grid-view");
  var galleryView = document.querySelector(".js-gallery-view");
  if (galleryView && !galleryView.classList.contains("hidden")) return galleryView;
  if (gridView && !gridView.classList.contains("hidden")) return gridView;
  return listView;
}
