// selection — row selection state and UI (single / ctrl / shift / touch
// multi-select), plus the batch selection bar. Depends on right-panel.js to
// render the selected item(s) detail. Selection state lives in the pure
// reducer in selection-state.js; this module is the thin DOM layer.
import { __t } from "../core/i18n.js";
import { createSelectionState, reduceSelection } from "./selection-state.js";
import { openRightPanel, resetRightPanel, openMultiSelectPanel, openQuickPreview } from "./right-panel.js";

// ─── Selection state ────────────────────────────────────────────────────
var selState = createSelectionState();
var suppressClick = false;   // Suppress synthetic click after long-press

export function getCurrentDir() {
  var input = document.querySelector('[name="current_dir"]');
  if (input && input.value) return input.value;
  var m = window.location.pathname.match(/\/files\/(.*)/);
  return m ? "/" + m[1] : "/";
}

export function getRepoId() {
  var meta = document.querySelector('meta[name="repo-id"]');
  return meta ? meta.content : "";
}

export function getSelectedItems() {
  var items = [];
  document.querySelectorAll(".js-entry-row.selected").forEach(function (r) {
    items.push({
      name: r.dataset.name,
      type: r.dataset.type,
      repoId: r.dataset.repoId,
      path: r.dataset.path,
    });
  });
  return items;
}

export function getSelectedCount() {
  return selState.selected.size;
}

export function getSelectedPaths() {
  return Array.from(selState.selected);
}

// Sync the .selected class on every row to match the current selection state.
function applySelectionToDom() {
  // Remove .selected from all rows (including hidden views).
  document.querySelectorAll(".js-entry-row.selected").forEach(function (row) {
    row.classList.remove("selected");
  });
  // Re-apply .selected to rows in the visible views that are selected.
  var visViews = document.querySelectorAll(
    ".js-file-list-view:not(.hidden), .js-file-grid-view:not(.hidden), .js-gallery-view:not(.hidden)"
  );
  visViews.forEach(function (view) {
    view.querySelectorAll(".js-entry-row").forEach(function (row) {
      if (row.dataset.name && selState.selected.has(row.dataset.name)) {
        row.classList.add("selected");
      }
    });
  });
}

// Return the .js-entry-row elements in the currently visible view(s). Select
// All must count only these rows — the hidden grid/gallery views render the
// same entries again, so counting all views breaks the "deselect on second
// click" branch (the selected set is deduplicated but the row count isn't).
function getVisibleRows() {
  var rows = [];
  var visViews = document.querySelectorAll(
    ".js-file-list-view:not(.hidden), .js-file-grid-view:not(.hidden), .js-gallery-view:not(.hidden)"
  );
  visViews.forEach(function (view) {
    view.querySelectorAll(".js-entry-row").forEach(function (row) {
      rows.push(row);
    });
  });
  return rows;
}

function updateSelectionBar() {
  // Auto-clear stale selection (e.g. after partial refresh)
  if (selState.selected.size > 0) {
    var selectedCount = document.querySelectorAll(".js-entry-row.selected").length;
    if (selectedCount === 0 && document.querySelectorAll(".js-entry-row").length > 0) {
      selState = { selected: new Set(), anchor: selState.anchor, touchMode: selState.touchMode };
    }
  }
  var count = selState.selected.size;
  var isSelected = count > 0;

  // Toggle selection info and action buttons in the view toggle bar
  var info = document.getElementById("js-selection-info");
  var actions = document.getElementById("js-selection-actions");
  if (info) info.classList.toggle("hidden", !isSelected);
  if (actions) actions.classList.toggle("hidden", !isSelected);

  if (isSelected) {
    var countEl = document.querySelector(".js-selection-count");
    if (countEl) countEl.textContent = count;
  }

  // Update Select All button text
  var selBtn = document.getElementById("js-select-all-btn");
  if (selBtn) {
    var visibleRows = getVisibleRows().length;
    selBtn.textContent = selState.selected.size === visibleRows ? __t('ui.deselect_all') : __t('ui.select_all');
  }
}

export function clearSelection() {
  selState = reduceSelection(selState, { type: "clear" });
  applySelectionToDom();
  updateSelectionBar();
  resetRightPanel();
}

// Row click — single select, Ctrl toggle, Shift range, or touch multi-select
document.addEventListener("click", function (e) {
  var row = e.target.closest(".js-entry-row");

  // Suppress synthetic click after long-press on touch devices
  if (suppressClick) {
    suppressClick = false;
    return;
  }

  // Click on empty space inside file list — clear selection
  if (!row) {
    if (e.target.closest("button, a, #js-select-all-btn, .js-sort-bar")) return;
    if (e.target.closest(".file-list-container")) {
      clearSelection();
    }
    return;
  }

  // Ignore clicks on links and buttons within rows
  if (e.target.closest("a") || e.target.closest("button")) return;

  var name = row.dataset.name;
  if (!name) return;

  // Shift range selection needs the ordered names of the visible view.
  var orderedNames = [];
  if (e.shiftKey) {
    var view = document.querySelector(
      ".js-file-list-view:not(.hidden), .js-file-grid-view:not(.hidden), .js-gallery-view:not(.hidden)"
    );
    if (view) {
      view.querySelectorAll(".js-entry-row").forEach(function (r) {
        if (r.dataset.name) orderedNames.push(r.dataset.name);
      });
    }
  }

  selState = reduceSelection(selState, {
    type: "click",
    name: name,
    shift: e.shiftKey,
    ctrl: e.ctrlKey || e.metaKey,
    orderedNames: orderedNames,
  });
  applySelectionToDom();
  updateSelectionBar();
  updateSelectionPanel();
});

// Row dblclick — open quick-preview modal for previewable files.
document.addEventListener("dblclick", function (e) {
  var row = e.target.closest(".js-entry-row");
  if (!row) return;
  // Ignore dblclicks on the filename link (navigates) or buttons.
  if (e.target.closest("a") || e.target.closest("button")) return;
  var name = row.dataset.name;
  if (!name) return;
  // Directories and non-previewable files do nothing.
  if (row.dataset.type === "dir") return;
  if (row.dataset.isPreviewable !== "true" &&
      row.dataset.isVideo !== "true" &&
      row.dataset.isAudio !== "true") return;
  // A dblclick fires two single clicks first; the second one toggles this
  // single-selected row off. Re-select it so it stays selected.
  selState = reduceSelection(selState, { type: "selectOne", name: name });
  applySelectionToDom();
  updateSelectionBar();
  updateSelectionPanel();
  openQuickPreview(row);
});

// Update right panel based on current selection state
function updateSelectionPanel() {
  var count = selState.selected.size;
  if (count === 0) {
    resetRightPanel();
    return;
  }
  if (count === 1) {
    var selRow = document.querySelector(".js-entry-row.selected");
    if (selRow) {
      var dlUrl = selRow.dataset.type !== "dir"
        ? "/repos/" + selRow.dataset.repoId + "/files/" + selRow.dataset.path + "?dl=1"
        : "";
      openRightPanel({
        name: selRow.dataset.name,
        type: selRow.dataset.type,
        starred: selRow.dataset.starred === "true",
        extension: selRow.dataset.extension,
        path: selRow.dataset.path,
        repoId: selRow.dataset.repoId,
        modifierEmail: selRow.dataset.modifierEmail,
        thumbnailUrl: selRow.dataset.thumbnailUrl,
        thumbnailUrlLarge: selRow.dataset.thumbnailUrlLarge,
        size: selRow.dataset.size,
        sizeDisplay: selRow.dataset.sizeDisplay,
        isPreviewable: selRow.dataset.isPreviewable === "true",
        isVideo: selRow.dataset.isVideo === "true",
        isAudio: selRow.dataset.isAudio === "true",
        downloadUrl: dlUrl,
        recordId: selRow.dataset.recordId,
      });
    }
    return;
  }
  // Multiple items selected
  openMultiSelectPanel(getSelectedItems());
}

// Select All / Deselect All button
document.addEventListener("click", function (e) {
  var btn = e.target.closest("#js-select-all-btn");
  if (!btn) return;

  var visibleRows = getVisibleRows();
  if (selState.selected.size === visibleRows.length) {
    // Deselect all
    clearSelection();
  } else {
    // Select all
    var names = [];
    visibleRows.forEach(function (row) {
      if (row.dataset.name) names.push(row.dataset.name);
    });
    selState = reduceSelection(selState, { type: "selectAll", names: names });
    applySelectionToDom();
    updateSelectionBar();
    // Show multi-select panel
    openMultiSelectPanel(getSelectedItems());
  }
});

document.addEventListener("click", function (e) {
  if (e.target.closest(".js-deselect-all")) {
    clearSelection();
  }
});

// ─── Touch selection support (long-press multi-select) ──────────────────
var touchLongPressTimer = null;
var touchStartTarget = null;
var TOUCH_LONG_PRESS_MS = 500;

document.addEventListener("touchstart", function (e) {
  var row = e.target.closest(".js-entry-row");
  if (!row) return;
  if (e.target.closest("a") || e.target.closest("button")) return;

  touchStartTarget = row;

  touchLongPressTimer = setTimeout(function () {
    // Long press detected — enter multi-select mode
    touchLongPressTimer = null;

    selState = reduceSelection(selState, { type: "setTouchMode", value: true });

    var name = row.dataset.name;
    if (!name) return;

    // Toggle this item
    selState = reduceSelection(selState, {
      type: "click",
      name: name,
      shift: false,
      ctrl: false,
      orderedNames: [],
    });
    applySelectionToDom();
    updateSelectionBar();
    updateSelectionPanel();

    suppressClick = true;

    // Haptic feedback if available
    if (navigator.vibrate) navigator.vibrate(20);
  }, TOUCH_LONG_PRESS_MS);
}, { passive: true });

document.addEventListener("touchmove", function (e) {
  // Cancel long press if user starts scrolling
  if (touchLongPressTimer) {
    clearTimeout(touchLongPressTimer);
    touchLongPressTimer = null;
  }
}, { passive: true });

document.addEventListener("touchend", function (e) {
  if (touchLongPressTimer) {
    clearTimeout(touchLongPressTimer);
    touchLongPressTimer = null;
  }
}, { passive: true });

// Escape key exits touch multi-select mode
document.addEventListener("keydown", function (e) {
  if (e.key === "Escape" && selState.touchMode) {
    clearSelection();
  }
});

// Sync .selected class to the currently visible view (called after view switch)
function syncSelectionView() {
  if (selState.selected.size === 0) return;
  applySelectionToDom();
  updateSelectionBar();
}

document.addEventListener("nanofile:viewchange", function () {
  syncSelectionView();
});
