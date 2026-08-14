// list — partial list refresh (refreshFileList) and pagination (load-more /
// infinite scroll). Dispatches "nanofile:list-refreshed" after a refresh so
// view.js can re-apply sort/view state without list.js importing it.
import { getSort, getTagFilter, getVisibleView, getVisibleViewContainer } from "./state.js";
import { buildListUrl } from "./urls.js";

// ─── Skeleton loading ────────────────────────────────────────────────────
var skeleton = document.querySelector(".js-skeleton");
var fileListContainer = document.querySelector(".file-list-container");

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

// ─── Partial refresh ─────────────────────────────────────────────────────
export async function refreshFileList() {
  // Render all three views (view=all) so view switching stays a pure
  // client-side toggle after a mutation. The _viewRefreshing counter
  // suppresses pagination while a refresh is in flight so the infinite-scroll
  // observer doesn't load page 2 into the soon-to-be-replaced DOM.
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
        // A full refresh replaces the list with page 1 — start at the top so
        // the user isn't dropped at the bottom of the re-sorted/reloaded list.
        var mainEl = document.querySelector("main");
        if (mainEl) mainEl.scrollTop = 0;
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

// ─── Load more (pagination) ──────────────────────────────────────────────
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
  // A partial refresh (upload/delete/sort) is replacing the list; don't load
  // page 2 into the old DOM that is about to be swapped out.
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

    // Extract the view container HTML from the partial response
    var parser = document.createElement("div");
    parser.innerHTML = html;
    var newContainer = parser.querySelector(
      view === "grid" ? ".js-file-grid-view" :
      view === "gallery" ? ".js-gallery-view" :
      ".js-file-list-view"
    );
    if (!newContainer) { btn.disabled = false; if (spinner) spinner.classList.add("hidden"); return; }

    // Append new content: rows for list/grid, month groups for gallery
    if (view === "gallery") {
      var groups = newContainer.querySelectorAll(".gallery-month-group");
      groups.forEach(function (g) { container.appendChild(g); });
    } else {
      var rows = newContainer.querySelectorAll(".js-entry-row");
      rows.forEach(function (row) { container.appendChild(row); });

      // DOM recycling: if more than 3 pages loaded, remove the oldest page
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

    // Update pagination state from the response
    container.dataset.page = newContainer.dataset.page || String(nextPage);
    container.dataset.hasMore = newContainer.dataset.hasMore || "false";
    container.dataset.total = newContainer.dataset.total || container.dataset.total;

    // Update the count display in the load-more bar
    var loadedCount = document.querySelector(".js-loaded-count");
    var totalCount = container.dataset.total;
    if (loadedCount) {
      var loadedTotal = parseInt(container.dataset.page, 10) * 200;
      loadedCount.textContent = Math.min(loadedTotal, parseInt(totalCount, 10));
    }

    // Hide the load-more bar if no more pages
    if (container.dataset.hasMore !== "true") {
      var bar = document.querySelector(".js-load-more-bar");
      if (bar) bar.classList.add("hidden");
    }
  } catch (_) { /* ignore */ }

  btn.disabled = false;
  if (spinner) spinner.classList.add("hidden");
}

// Load more button click handler
document.addEventListener("click", function (e) {
  if (e.target.closest(".js-load-more-btn")) {
    loadMoreEntries();
  }
});

// ─── Infinite scroll ─────────────────────────────────────────────────────
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

// Fallback scroll listener on <main> (runs once; <main> is not replaced on refresh)
if (!("IntersectionObserver" in window)) {
  var mainEl = document.querySelector("main");
  if (mainEl) {
    mainEl.addEventListener("scroll", onFileListScroll, { passive: true });
  }
}
initInfiniteScroll();

// Keep the pagination bar in sync with the visible view after a view change.
document.addEventListener("nanofile:viewchange", function () {
  syncPaginationBar();
});
