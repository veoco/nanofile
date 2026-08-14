// selection — row selection state and UI (single / ctrl / shift / touch
// multi-select), plus the batch selection bar. Depends on right-panel.js to
// render the selected item(s) detail.
import { __t } from "../core/i18n.js";
import { openRightPanel, resetRightPanel, openMultiSelectPanel, openQuickPreview } from "./right-panel.js";

// ─── Selection state ────────────────────────────────────────────────────
var selectedPaths = new Set();
var anchorPath = null;       // Anchor for Shift+click range selection
var touchSelectMode = false; // Touch multi-select mode (long-press activated)
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
  return selectedPaths.size;
}

export function getSelectedPaths() {
  return Array.from(selectedPaths);
}

function updateSelectionBar() {
  // Auto-clear stale selection (e.g. after partial refresh)
  if (selectedPaths.size > 0) {
    var selectedCount = document.querySelectorAll(".js-entry-row.selected").length;
    if (selectedCount === 0 && document.querySelectorAll(".js-entry-row").length > 0) {
      selectedPaths.clear();
    }
  }
  var count = selectedPaths.size;
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
    var totalRows = document.querySelectorAll(".js-entry-row").length;
    selBtn.textContent = selectedPaths.size === totalRows ? __t('ui.deselect_all') : __t('ui.select_all');
  }
}

export function clearSelection() {
  touchSelectMode = false;
  selectedPaths.clear();
  document.querySelectorAll(".js-entry-row.selected").forEach(function (row) {
    row.classList.remove("selected");
  });
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
      touchSelectMode = false;
      clearSelection();
    }
    return;
  }

  // Ignore clicks on links and buttons within rows
  if (e.target.closest("a") || e.target.closest("button")) return;

  var name = row.dataset.name;
  if (!name) return;

  // ── Shift+click: range select from anchor to clicked item ──
  if (e.shiftKey) {
    var view = document.querySelector(
      ".js-file-list-view:not(.hidden), .js-file-grid-view:not(.hidden), .js-gallery-view:not(.hidden)"
    );
    if (view && anchorPath) {
      var rows = view.querySelectorAll(".js-entry-row");
      var anchorIdx = -1, currentIdx = -1;
      for (var i = 0; i < rows.length; i++) {
        var dn = rows[i].dataset.name;
        if (dn === anchorPath) anchorIdx = i;
        if (dn === name) currentIdx = i;
      }
      if (anchorIdx !== -1 && currentIdx !== -1) {
        // Clear current selection and select range
        clearSelection();
        var start = Math.min(anchorIdx, currentIdx);
        var end = Math.max(anchorIdx, currentIdx);
        for (var i = start; i <= end; i++) {
          var n = rows[i].dataset.name;
          if (n) {
            selectedPaths.add(n);
            rows[i].classList.add("selected");
          }
        }
        updateSelectionBar();
        updateSelectionPanel();
        return;
      }
    }
    // Fallback: anchor not found or no view — single select
    clearSelection();
    selectedPaths.add(name);
    row.classList.add("selected");
    anchorPath = name;
    updateSelectionBar();
    updateSelectionPanel();
    return;
  }

  // ── Ctrl+click: toggle this item ──
  if (e.ctrlKey || e.metaKey) {
    if (selectedPaths.has(name)) {
      selectedPaths.delete(name);
      row.classList.remove("selected");
    } else {
      selectedPaths.add(name);
      row.classList.add("selected");
    }

  // ── Touch multi-select: toggle like Ctrl+click ──
  } else if (touchSelectMode) {
    if (selectedPaths.has(name)) {
      selectedPaths.delete(name);
      row.classList.remove("selected");
    } else {
      selectedPaths.add(name);
      row.classList.add("selected");
    }

  // ── Normal click: update anchor, single select ──
  } else {
    anchorPath = name;
    if (selectedPaths.size === 1 && selectedPaths.has(name)) {
      // Clicking the only selected item — deselect it
      selectedPaths.delete(name);
      row.classList.remove("selected");
    } else {
      clearSelection();
      selectedPaths.add(name);
      row.classList.add("selected");
    }
  }

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
  clearSelection();
  selectedPaths.add(name);
  row.classList.add("selected");
  updateSelectionBar();
  updateSelectionPanel();
  openQuickPreview(row);
});

// Update right panel based on current selection state
function updateSelectionPanel() {
  var count = selectedPaths.size;
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

  var totalRows = document.querySelectorAll(".js-entry-row");
  if (selectedPaths.size === totalRows.length) {
    // Deselect all
    clearSelection();
  } else {
    // Select all
    selectedPaths.clear();
    totalRows.forEach(function (row) {
      var name = row.dataset.name;
      if (name) {
        selectedPaths.add(name);
        row.classList.add("selected");
      }
    });
    updateSelectionBar();
    // Show multi-select panel
    openMultiSelectPanel(getSelectedItems());
  }
});

document.addEventListener("click", function (e) {
  if (e.target.closest(".js-deselect-all")) {
    touchSelectMode = false;
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
    touchSelectMode = true;
    touchLongPressTimer = null;

    var name = row.dataset.name;
    if (!name) return;

    // Toggle this item
    if (selectedPaths.has(name)) {
      selectedPaths.delete(name);
      row.classList.remove("selected");
    } else {
      selectedPaths.add(name);
      row.classList.add("selected");
    }
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
  if (e.key === "Escape" && touchSelectMode) {
    touchSelectMode = false;
    clearSelection();
  }
});

// Sync .selected class to the currently visible view (called after view switch)
function syncSelectionView() {
  if (selectedPaths.size === 0) return;
  // Remove .selected from all rows (including hidden views)
  document.querySelectorAll(".js-entry-row.selected").forEach(function (row) {
    row.classList.remove("selected");
  });
  // Re-apply .selected to rows in the visible view that match selectedPaths
  var visViews = document.querySelectorAll(
    ".js-file-list-view:not(.hidden), .js-file-grid-view:not(.hidden), .js-gallery-view:not(.hidden)"
  );
  visViews.forEach(function (view) {
    view.querySelectorAll(".js-entry-row").forEach(function (row) {
      if (selectedPaths.has(row.dataset.name)) {
        row.classList.add("selected");
      }
    });
  });
  updateSelectionBar();
}

document.addEventListener("nanofile:viewchange", function () {
  syncSelectionView();
});
