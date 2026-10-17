// list — partial list refresh (refreshFileList) and pagination. Dispatches
// "nanofile:list-refreshed" so view.js can re-apply sort/view state.
import { getSort, getTagFilter, getVisibleView, getVisibleViewContainer } from "./state.js";
import { buildListUrl } from "./urls.js";

var skeleton = document.querySelector(".js-skeleton");
var fileListContainer = document.querySelector(".file-list-container");

// The scrolling region is the file-list viewport, not <main>: the breadcrumb
// and the action toolbar are pinned chrome above it.
function listScroller() {
  return document.getElementById("nf-list-scroll");
}

export function showFileSkeleton() {
  if (skeleton) skeleton.classList.remove("hidden");
  if (fileListContainer) {
    var list = fileListContainer.querySelector(".js-file-list-view");
    if (list) list.classList.add("hidden");
  }
}

export function hideFileSkeleton() {
  if (skeleton) skeleton.classList.add("hidden");
  if (fileListContainer) {
    var list = fileListContainer.querySelector(".js-file-list-view");
    if (list) list.classList.remove("hidden");
  }
}

export async function refreshFileList() {
// Render all three views so view switching stays a pure client-side toggle after a mutation;
// `_viewRefreshing` suppresses pagination while a refresh is in flight.
  window._viewRefreshing = (window._viewRefreshing || 0) + 1;
  var url = buildListUrl({
    pathname: window.location.pathname,
    view: "all",
    page: null,
    sort: getSort(),
    tag: getTagFilter(),
  });
  try {
    var resp = await fetch(url);
    if (resp.ok) {
      var html = await resp.text();
      var container = document.querySelector(".file-list-container");
      if (container) {
        container.outerHTML = html;
        document.dispatchEvent(new CustomEvent("nanofile:list-refreshed"));
        initInfiniteScroll();
// A full refresh starts at page 1, so do not leave the user at the bottom.
        var scroller = listScroller();
        if (scroller) scroller.scrollTop = 0;
      } else {
        window.location.reload();
      }
    } else {
      window.location.reload();
    }
  } catch (_) {
    window.location.reload();
  } finally {
    window._viewRefreshing = Math.max(0, (window._viewRefreshing || 1) - 1);
  }
}

export function syncPaginationBar() {
  var container = getVisibleViewContainer();
  if (!container) return;
  var bar = document.querySelector(".js-load-more-bar");
  if (!bar) return;
  var loadedCount = document.querySelector(".js-loaded-count");
  var hasMore = container.dataset.hasMore === "true";
  bar.classList.toggle("hidden", !hasMore);
  if (loadedCount && container.dataset.total) {
    var page = parseInt(container.dataset.page || "1", 10);
    var total = parseInt(container.dataset.total, 10);
    loadedCount.textContent = Math.min(page * 200, total);
  }
}

export async function loadMoreEntries() {
// A partial refresh replaces the list; do not load page 2 into the old DOM.
  if (window._viewRefreshing) return;
  var container = getVisibleViewContainer();
  if (!container) return;
  var btn = document.querySelector(".js-load-more-btn");
  var spinner = document.querySelector(".js-load-more-spinner");
  if (!btn || btn.disabled) return;

  var page = parseInt(container.dataset.page || "1", 10);
  var hasMore = container.dataset.hasMore === "true";
  if (!hasMore) return;

  btn.disabled = true;
  if (spinner) spinner.classList.remove("hidden");

  var view = getVisibleView();
  var nextPage = page + 1;
  var url = buildListUrl({
    pathname: window.location.pathname,
    view: view,
    page: nextPage,
    sort: getSort(),
    tag: getTagFilter(),
  });

  try {
    var resp = await fetch(url);
    if (!resp.ok) { btn.disabled = false; if (spinner) spinner.classList.add("hidden"); return; }
    var html = await resp.text();

    var parser = document.createElement("div");
    parser.innerHTML = html;
    var newContainer = parser.querySelector(
      view === "grid" ? ".js-file-grid-view" :
      view === "gallery" ? ".js-gallery-view" :
      ".js-file-list-view"
    );
    if (!newContainer) { btn.disabled = false; if (spinner) spinner.classList.add("hidden"); return; }

// Append new content. A page boundary usually falls inside a month, so merge the incoming
// first group into the last on-screen group when the heading matches, or the month heading
// prints twice and its count splits.
    if (view === "gallery") {
      // The groups the template renders are `.nf-gal-group`; this used to read
      // the gallery's pre-rebuild class name, matched nothing, and so the
      // gallery never grew past its first page.
      // A page boundary usually falls inside a month, so the incoming page's
      // first group is the tail of the month already on screen; appending it
      // as-is would print that month's heading twice and split its count, so
      // merge its tiles into the last group when the heading matches.
      newContainer.querySelectorAll(".nf-gal-group").forEach(function (g) {
        var existing = container.querySelectorAll(".nf-gal-group");
        var last = existing[existing.length - 1];
        var grid = last && last.querySelector(".nf-gal-grid");
        var incoming = g.querySelector(".nf-gal-grid");
        var lastHead = last && last.querySelector(".nf-gal-head h2");
        var head = g.querySelector(".nf-gal-head h2");
        var sameMonth =
          !!lastHead && !!head && lastHead.textContent.trim() === head.textContent.trim();
        if (!sameMonth || !grid || !incoming) {
          container.appendChild(g);
          return;
        }
        incoming.querySelectorAll(".js-entry-row").forEach(function (tile) {
          grid.appendChild(tile);
        });
        var count = last.querySelector(".nf-gal-n");
        if (count) count.textContent = String(grid.querySelectorAll(".js-entry-row").length);
      });
    } else {
      var rows = newContainer.querySelectorAll(".js-entry-row");
      rows.forEach(function (row) { container.appendChild(row); });

      var allRows = container.querySelectorAll(".js-entry-row");
      if (allRows.length > 600) {
        var oldestPage = Infinity;
        allRows.forEach(function (r) {
          var p = parseInt(r.dataset.page, 10);
          if (p < oldestPage) oldestPage = p;
        });
        if (oldestPage < nextPage) {
          var toRemove = container.querySelectorAll('.js-entry-row[data-page="' + oldestPage + '"]');
          toRemove.forEach(function (r) { r.remove(); });
        }
      }
    }

    container.dataset.page = newContainer.dataset.page || String(nextPage);
    container.dataset.hasMore = newContainer.dataset.hasMore || "false";
    container.dataset.total = newContainer.dataset.total || container.dataset.total;

    var loadedCount = document.querySelector(".js-loaded-count");
    var totalCount = container.dataset.total;
    if (loadedCount) {
      var loadedTotal = parseInt(container.dataset.page, 10) * 200;
      loadedCount.textContent = Math.min(loadedTotal, parseInt(totalCount, 10));
    }

    if (container.dataset.hasMore !== "true") {
      var bar = document.querySelector(".js-load-more-bar");
      if (bar) bar.classList.add("hidden");
    }
  } catch (_) { /* ignore */ }

  btn.disabled = false;
  if (spinner) spinner.classList.add("hidden");
}

document.addEventListener("click", function (e) {
  if (e.target.closest(".js-load-more-btn")) {
    loadMoreEntries();
  }
});

var infiniteScrollTimer = null;
function onFileListScroll() {
  if (infiniteScrollTimer) return;
  infiniteScrollTimer = setTimeout(function () {
    infiniteScrollTimer = null;
    var bar = document.querySelector(".js-load-more-bar");
    if (!bar || bar.classList.contains("hidden")) return;
    var view = getVisibleViewContainer();
    if (!view) return;
    var rect = view.getBoundingClientRect();
    // Trigger when the view bottom is within 300px of the viewport bottom
    if (rect.bottom - window.innerHeight < 300) {
      loadMoreEntries();
    }
  }, 200);
}

// Reusable — also called after DOM refresh so the observer reconnects
var fileListObserver = null;
export function initInfiniteScroll() {
  if (fileListObserver) { fileListObserver.disconnect(); fileListObserver = null; }
  var loadMoreBar = document.querySelector(".js-load-more-bar");
  if (loadMoreBar && "IntersectionObserver" in window) {
    fileListObserver = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          loadMoreEntries();
        }
      });
    }, { rootMargin: "300px" });
    fileListObserver.observe(loadMoreBar);
  }
}

// Fallback scroll listener on the list viewport (runs once; refresh does not replace it).
if (!("IntersectionObserver" in window)) {
  var mainEl = listScroller();
  if (mainEl) {
    mainEl.addEventListener("scroll", onFileListScroll, { passive: true });
  }
}
initInfiniteScroll();

// Keep the pagination bar in sync with the visible view after a view change.
document.addEventListener("nanofile:viewchange", function () {
  syncPaginationBar();
});
