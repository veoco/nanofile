// operations — file-browser mutations and dialogs: delete, history, share,
// rename, batch delete/move/copy, directory picker, zip download, reindex.
import { __t } from "../core/i18n.js";
import { getCookie, escapeHtml, escapeAttr } from "../core/utils.js";
import { apiFetch } from "../core/api.js";
import { Toast } from "../core/toast.js";
import { ConfirmDialog } from "../core/confirm.js";
import { refreshFileList } from "./list.js";
import { getCurrentDir, getRepoId, getSelectedItems, getSelectedPaths, getSelectedCount, clearSelection } from "./selection.js";

// ─── Delete file/dir via API ────────────────────────────────────────────
document.addEventListener("click", async function (e) {
  const btn = e.target.closest(".js-delete-btn");
  if (!btn) return;

  var repoId = btn.dataset.repoId;
  var path = btn.dataset.path;
  var name = btn.dataset.name;
  var entryType = btn.dataset.type;

  var confirmed = await ConfirmDialog.confirm(
    __t('ui.delete'),
    __t('ui.confirm_delete_named', { name: name }),
    { confirmText: __t('ui.delete'), variant: "danger" }
  );
  if (!confirmed) return;

  var csrfToken = getCookie("sfcsrftoken");
  if (!csrfToken) {
    window.location.href = "/accounts/login/";
    return;
  }

  var apiPath = entryType === "dir"
    ? "/api2/repos/" + repoId + "/dir/?p=" + encodeURIComponent(path)
    : "/api2/repos/" + repoId + "/file/?p=" + encodeURIComponent(path);

  try {
    var res = await fetch(apiPath, {
      method: "DELETE",
      headers: { "X-CSRFToken": csrfToken },
    });
    if (res.ok) {
      clearSelection();
      refreshFileList();
    } else {
      var text = await res.text().catch(function () { return res.statusText; });
      Toast.error(__t('ui.delete_failed', { msg: text }));
    }
  } catch (err) {
    Toast.error(__t('ui.delete_failed', { msg: err.message }));
  }
});

// ─── File history dialog ────────────────────────────────────────────────
var historyOverlay = document.getElementById("history-dialog-overlay");
var historyPathEl = historyOverlay
  ? historyOverlay.querySelector(".js-history-path")
  : null;
var historyListEl = historyOverlay
  ? historyOverlay.querySelector(".js-history-list")
  : null;

function formatHistoryTime(ts) {
  var d = new Date(ts * 1000);
  return d.toLocaleString();
}

function formatHistorySize(n) {
  if (n >= 1024 * 1024 * 1024) return (n / (1024 * 1024 * 1024)).toFixed(1) + " GB";
  if (n >= 1024 * 1024) return (n / (1024 * 1024)).toFixed(1) + " MB";
  if (n >= 1024) return (n / 1024).toFixed(1) + " KB";
  return n + " B";
}

function closeHistoryDialog() {
  if (historyOverlay) historyOverlay.classList.add("hidden");
}

async function openHistoryDialog(repoId, path) {
  if (!historyOverlay) return;
  historyPathEl.textContent = path;
  historyOverlay.classList.remove("hidden");
  historyListEl.innerHTML =
    '<div class="text-sm text-gray-400 text-center py-4">Loading...</div>';

  try {
    var res = await apiFetch(
      "/api/v2.1/repos/" +
        encodeURIComponent(repoId) +
        "/file/history/?p=" +
        encodeURIComponent(path)
    );
    var body = await res.json();
    renderHistoryList(body.data || [], repoId, path);
  } catch (err) {
    historyListEl.innerHTML =
      '<div class="text-sm text-red-500 text-center py-4">Failed to load history: ' +
      escapeHtml(err.message) +
      "</div>";
  }
}

function renderHistoryList(items, repoId, path) {
  if (!items.length) {
    historyListEl.innerHTML =
      '<div class="text-sm text-gray-400 text-center py-4">No history available</div>';
    return;
  }
  var html = "";
  items.forEach(function (item, idx) {
    var versionNo = items.length - idx;
    var commitId = item.commit_id || "";
    var fileName = path.split("/").pop() || "";
    var revUrl =
      "/api/v2.1/repos/" +
      encodeURIComponent(repoId) +
      "/file/revision/?p=" +
      encodeURIComponent(path) +
      "&commit_id=" +
      encodeURIComponent(commitId);
    html +=
      '<div class="flex items-center justify-between gap-2 px-2 py-2 rounded-md hover:bg-gray-50 dark:hover:bg-surface-700">' +
      '<div class="min-w-0">' +
      '<div class="text-sm font-medium text-gray-800 dark:text-gray-200">Version ' +
      versionNo +
      '<span class="ml-2 text-xs font-normal text-gray-400">' +
      escapeHtml(item.last_modified_by || "") +
      "</span></div>" +
      '<div class="text-xs text-gray-400">' +
      formatHistoryTime(item.mtime || item.file_mtime || 0) +
      " · " +
      formatHistorySize(item.size || item.file_size || 0) +
      "</div></div>" +
      '<div class="flex items-center gap-1 flex-shrink-0">' +
      '<a href="' +
      revUrl +
      '" download class="px-2 py-1 rounded-md text-xs font-medium text-brand-600 dark:text-brand-400 border border-brand-200 dark:border-brand-900/50 hover:bg-brand-50 dark:hover:bg-brand-900/20">Download</a>' +
      '<button type="button" class="js-history-restore px-2 py-1 rounded-md text-xs font-medium text-gray-600 dark:text-gray-300 border border-gray-200 dark:border-gray-600 hover:bg-gray-50 dark:hover:bg-surface-700" data-commit-id="' +
      escapeAttr(commitId) +
      '" data-repo-id="' +
      escapeAttr(repoId) +
      '" data-path="' +
      escapeAttr(path) +
      '" data-name="' +
      escapeAttr(fileName) +
      '">Restore</button>' +
      "</div></div>";
  });
  historyListEl.innerHTML = html;
}

// Restore action (event delegation inside the history list)
if (historyListEl) {
  historyListEl.addEventListener("click", async function (e) {
    var btn = e.target.closest(".js-history-restore");
    if (!btn) return;
    var repoId = btn.dataset.repoId;
    var path = btn.dataset.path;
    var commitId = btn.dataset.commitId;
    var name = btn.dataset.name;

    var confirmed = await ConfirmDialog.confirm(
      __t('ui.restore_version'),
      __t('ui.confirm_restore_version', { name: name }),
      { confirmText: __t('ui.restore') }
    );
    if (!confirmed) return;

    try {
      await apiFetch(
        "/api/v2.1/repos/" +
          encodeURIComponent(repoId) +
          "/file/revision/restore/?p=" +
          encodeURIComponent(path) +
          "&commit_id=" +
          encodeURIComponent(commitId),
        { method: "POST" }
      );
      Toast.success(__t('ui.restored', { name: name }));
      closeHistoryDialog();
      refreshFileList();
    } catch (err) {
      Toast.error(__t('ui.restore_failed', { msg: err.message }));
    }
  });
}

// Open history from any .js-history-btn
document.addEventListener("click", function (e) {
  var btn = e.target.closest(".js-history-btn");
  if (!btn) return;
  openHistoryDialog(btn.dataset.repoId, btn.dataset.path);
});

// Close via backdrop, close button, or Escape
document.addEventListener("click", function (e) {
  if (!historyOverlay || historyOverlay.classList.contains("hidden")) return;
  if (e.target === historyOverlay) closeHistoryDialog();
});
if (historyOverlay) {
  historyOverlay.querySelectorAll(".js-history-close").forEach(function (btn) {
    btn.addEventListener("click", closeHistoryDialog);
  });
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape" && !historyOverlay.classList.contains("hidden")) {
      closeHistoryDialog();
    }
  });
}

// ─── Share dialog ───────────────────────────────────────────────────────
var shareDialog = document.getElementById("share-dialog-overlay");
var shareDialogPath = document.querySelector(".js-share-dialog-path");
var sharePasswordInput = document.getElementById("share-password-input");
var shareExpirySelect = document.getElementById("share-expiry-select");
var shareDescriptionInput = document.getElementById("share-description-input");
var shareDialogError = document.querySelector(".js-share-dialog-error");
var shareConfirmBtn = document.querySelector(".js-share-confirm");
var shareCancelBtn = document.querySelector(".js-share-cancel");
var shareDeleteBtn = document.getElementById("share-delete-btn");
var shareLinkDisplay = document.getElementById("share-link-display");
var shareLinkUrl = document.getElementById("share-link-url");
var shareCreateForm = shareDialog ? shareDialog.querySelector(".space-y-3") : null;
var shareCurrentRepoId = "";
var shareCurrentPath = "";
var shareCurrentType = "";
var shareCurrentToken = "";

document.addEventListener("click", function (e) {
  const btn = e.target.closest(".js-share-btn");
  if (!btn) return;
  if (!shareDialog) return;

  shareCurrentRepoId = btn.dataset.repoId;
  shareCurrentPath = btn.dataset.path;
  shareCurrentType = btn.dataset.type || "file";
  shareCurrentToken = "";
  var name = shareCurrentPath.split("/").filter(Boolean).pop() || shareCurrentPath;

  if (!shareCurrentRepoId || !shareCurrentPath) return;

  // Reset dialog
  sharePasswordInput.value = "";
  shareExpirySelect.value = "";
  shareDescriptionInput.value = "";
  shareDialogError.classList.add("hidden");
  shareDialogError.textContent = "";
  shareConfirmBtn.disabled = false;
  shareConfirmBtn.textContent = __t('ui.create');
  shareConfirmBtn.classList.remove("hidden");
  shareCancelBtn.classList.remove("hidden");
  shareDeleteBtn.classList.add("hidden");
  if (shareCreateForm) shareCreateForm.classList.remove("hidden");
  if (shareLinkDisplay) shareLinkDisplay.classList.add("hidden");

  // Show path
  var displayName = btn.dataset.name || name;
  shareDialogPath.textContent = displayName;
  shareDialog.classList.remove("hidden");
});

if (shareDialog) {
  shareCancelBtn.addEventListener("click", function () {
    shareDialog.classList.add("hidden");
  });
  shareDialog.addEventListener("click", function (e) {
    if (e.target === shareDialog) shareDialog.classList.add("hidden");
  });

  shareDeleteBtn.addEventListener("click", async function () {
    if (!shareCurrentToken) return;
    if (!confirm(__t('ui.confirm_delete_share'))) return;

    shareDeleteBtn.disabled = true;
    shareDialogError.classList.add("hidden");

    try {
      var resp = await apiFetch("/api/v2.1/share-links/" + shareCurrentToken + "/", {
        method: "DELETE",
      });
      if (resp.ok) {
        shareCurrentToken = "";
        if (shareCreateForm) shareCreateForm.classList.remove("hidden");
        if (shareLinkDisplay) shareLinkDisplay.classList.add("hidden");
        shareDeleteBtn.classList.add("hidden");
        shareConfirmBtn.textContent = __t('ui.create');
        shareCancelBtn.classList.remove("hidden");
        shareDialogError.textContent = "";
      } else {
        var text = await resp.text().catch(function () { return ""; });
        shareDialogError.textContent = text || __t('ui.delete_share_failed');
        shareDialogError.classList.remove("hidden");
      }
    } catch (err) {
      shareDialogError.textContent = err.message;
      shareDialogError.classList.remove("hidden");
    } finally {
      shareDeleteBtn.disabled = false;
    }
  });

  shareConfirmBtn.addEventListener("click", async function () {
    // If in "Close" mode, just close the dialog
    if (shareConfirmBtn.textContent === "Close") {
      shareDialog.classList.add("hidden");
      return;
    }

    var body = {
      repo_id: shareCurrentRepoId,
      path: shareCurrentPath,
    };

    var password = sharePasswordInput.value.trim();
    if (password) body.password = password;

    var expireDays = shareExpirySelect.value;
    if (expireDays) body.expire_days = parseInt(expireDays, 10);

    var description = shareDescriptionInput.value.trim();
    if (description) body.description = description;

    shareConfirmBtn.disabled = true;
    shareConfirmBtn.textContent = "Creating...";
    shareDialogError.classList.add("hidden");

    try {
      var resp = await apiFetch("/api/v2.1/share-links/", {
        method: "POST",
        body: JSON.stringify(body),
      });
      var data = await resp.json();
      shareCurrentToken = data.token;
      var sType = data.s_type || shareCurrentType;
      var prefix = sType === "d" ? "/d/" : "/f/";
      var shareUrl = window.location.origin + prefix + data.token + "/";
      // Show URL in dialog instead of closing
      if (shareCreateForm) shareCreateForm.classList.add("hidden");
      if (shareLinkDisplay) {
        shareLinkUrl.value = shareUrl;
        shareLinkDisplay.classList.remove("hidden");
      }
      shareConfirmBtn.textContent = "Close";
      shareCancelBtn.classList.add("hidden");
      shareDeleteBtn.classList.remove("hidden");
    } catch (err) {
      shareDialogError.textContent = err.message;
      shareDialogError.classList.remove("hidden");
    } finally {
      shareConfirmBtn.disabled = false;
      // Don't reset to "Create" if in Close mode (success path sets it to Close)
      if (shareConfirmBtn.textContent !== "Close") {
        shareConfirmBtn.textContent = __t('ui.create');
      }
    }
  });
}

// ─── Rename dialog ──────────────────────────────────────────────────────
const renameOverlay = document.getElementById("rename-overlay");
const renameOldPath = document.getElementById("rename-old-path");
const renameInput = document.getElementById("rename-input");

document.addEventListener("click", function (e) {
  const btn = e.target.closest(".js-rename-btn");
  if (!btn) return;

  renameOldPath.value = btn.dataset.path;
  renameInput.value = btn.dataset.name;
  renameOverlay.classList.remove("hidden");
  setTimeout(function () {
    renameInput.focus();
    renameInput.select();
  }, 100);
});

if (renameOverlay) {
  renameOverlay.addEventListener("click", function (e) {
    if (e.target === renameOverlay) renameOverlay.classList.add("hidden");
  });
}

document.addEventListener("keydown", function (e) {
  if (e.key === "Escape" && renameOverlay && !renameOverlay.classList.contains("hidden")) {
    renameOverlay.classList.add("hidden");
  }
});

const renameCancel = document.querySelector(".js-rename-cancel");
if (renameCancel) {
  renameCancel.addEventListener("click", function () {
    renameOverlay.classList.add("hidden");
  });
}

const renameForm = document.getElementById("rename-dialog-form");
if (renameForm) {
  renameForm.addEventListener("submit", async function (e) {
    var newName = renameInput.value.trim();
    if (!newName) {
      e.preventDefault();
      return;
    }
    e.preventDefault();
    renameOverlay.classList.add("hidden");

    // Extract repo_id from form action URL: /libraries/{id}/files/rename/
    var repoId = renameForm.action.match(/\/libraries\/([^/]+)\//)[1];
    var oldPath = renameOldPath.value;

    // Determine entry type from the rename button's data attribute
    var renameBtn = document.querySelector('.js-rename-btn[data-path="' + oldPath + '"]');
    var entryType = renameBtn ? renameBtn.dataset.type || "file" : "file";

    var apiPath =
      entryType === "dir"
        ? "/api2/repos/" + repoId + "/dir/?p=" + encodeURIComponent(oldPath)
        : "/api2/repos/" + repoId + "/file/?p=" + encodeURIComponent(oldPath);

    try {
      var formBody = new URLSearchParams();
      formBody.append("operation", "rename");
      formBody.append("newname", newName);

      var res = await apiFetch(apiPath, {
        method: "POST",
        body: formBody.toString(),
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
      });
      Toast.success(__t('ui.renamed_to', { name: newName }));
      refreshFileList();
    } catch (err) {
      Toast.error(__t('ui.rename_failed', { msg: err.message }));
    }
  });
}

// ─── Batch delete ───────────────────────────────────────────────────────
document.addEventListener("click", async function (e) {
  var btn = e.target.closest(".js-batch-delete");
  if (!btn) return;
  if (getSelectedCount() === 0) return;

  var confirmed = await ConfirmDialog.confirm(
    __t('ui.delete'),
    __t('ui.confirm_delete_n_items', { n: getSelectedCount() }),
    { confirmText: __t('ui.delete'), variant: "danger" }
  );
  if (!confirmed) return;

  var repoId = getRepoId();
  if (!repoId) { Toast.error(__t('ui.cannot_determine_repo')); return; }
  var parentDir = getCurrentDir();
  if (!parentDir) { Toast.error(__t('ui.cannot_determine_parent')); return; }

  try {
    await apiFetch("/api/v2.1/repos/batch-delete-item/", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        repo_id: repoId,
        parent_dir: parentDir,
        dirents: getSelectedPaths(),
      }),
    });
    Toast.success(__t('ui.deleted_n_items', { n: getSelectedCount() }));
    clearSelection();
    refreshFileList();
  } catch (err) {
    Toast.error(__t('ui.batch_delete_failed', { msg: err.message }));
  }
});

// ─── Directory picker (for batch move/copy) ─────────────────────────────
var pickerOperation = null;  // "move" or "copy"
var pickerPath = "/";

function openDirPicker(operation) {
  pickerOperation = operation;
  pickerPath = getCurrentDir();

  var titleEl = document.getElementById("dir-picker-title");
  if (titleEl) {
    titleEl.textContent = __t(operation === "move" ? 'ui.move_title' : 'ui.copy_title', { n: getSelectedCount() });
  }

  var confirmBtn = document.querySelector(".js-picker-confirm");
  if (confirmBtn) {
    confirmBtn.textContent = operation === "move" ? __t('ui.move_here') : __t('ui.copy_here');
  }

  var overlay = document.getElementById("dir-picker-overlay");
  if (!overlay) return;
  overlay.classList.remove("hidden");
  loadPickerDirectory(pickerPath);
}

function closeDirPicker() {
  var overlay = document.getElementById("dir-picker-overlay");
  if (overlay) overlay.classList.add("hidden");
  pickerOperation = null;
}

async function loadPickerDirectory(path) {
  var listEl = document.getElementById("dir-picker-list");
  var breadcrumbEl = document.getElementById("dir-picker-breadcrumb");
  if (!listEl || !breadcrumbEl) return;

  listEl.innerHTML = '<div class="text-sm text-gray-400 text-center py-4">Loading...</div>';
  pickerPath = path;
  renderPickerBreadcrumb(path, breadcrumbEl);

  var repoId = getRepoId();
  if (!repoId) { listEl.innerHTML = '<div class="text-sm text-red-500 text-center py-4">Error: no repo</div>'; return; }

  try {
    var resp = await fetch("/api2/repos/" + encodeURIComponent(repoId) + "/dir/?p=" + encodeURIComponent(path));
    if (!resp.ok) throw new Error(resp.statusText);
    var entries = await resp.json();
    // Filter to directories only
    var dirs = entries.filter(function (e) { return e.type === "dir"; });
    renderPickerDirList(dirs, listEl);
  } catch (err) {
    listEl.innerHTML = '<div class="text-sm text-red-500 text-center py-4">Failed to load: ' + escapeHtml(err.message) + '</div>';
  }
}

function renderPickerBreadcrumb(path, breadcrumbEl) {
  var parts = path.split("/").filter(Boolean);
  var html = '<button class="js-picker-nav px-1.5 py-0.5 rounded hover:bg-gray-100 dark:hover:bg-surface-700" data-path="/">/</button>';
  var accum = "";
  for (var i = 0; i < parts.length; i++) {
    accum += "/" + parts[i];
    html += '<span class="text-gray-300 dark:text-gray-600">/</span>';
    html += '<button class="js-picker-nav px-1.5 py-0.5 rounded hover:bg-gray-100 dark:hover:bg-surface-700" data-path="' + escapeAttr(accum) + '">' + escapeHtml(parts[i]) + '</button>';
  }
  breadcrumbEl.innerHTML = html;
}

function renderPickerDirList(dirs, listEl) {
  if (dirs.length === 0) {
    listEl.innerHTML = '<div class="text-sm text-gray-400 text-center py-4">No subdirectories</div>';
    return;
  }
  listEl.innerHTML = dirs.map(function (d) {
    return '<div class="js-picker-dir flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer hover:bg-gray-100 dark:hover:bg-surface-700 text-sm text-gray-700 dark:text-gray-300" data-path="' + escapeAttr(d.path || d.name) + '">' +
      '<svg class="h-4 w-4 text-amber-500 flex-shrink-0" fill="currentColor" viewBox="0 0 24 24"><path d="M2 6a2 2 0 012-2h5l2 2h9a2 2 0 012 2v10a2 2 0 01-2 2H4a2 2 0 01-2-2V6z"/></svg>' +
      '<span class="truncate">' + escapeHtml(d.name) + '</span>' +
      '</div>';
  }).join("");
}

// Open move/copy picker
document.addEventListener("click", function (e) {
  var btn = e.target.closest(".js-batch-move, .js-batch-copy");
  if (!btn) return;
  if (getSelectedCount() === 0) return;
  var operation = btn.classList.contains("js-batch-move") ? "move" : "copy";
  openDirPicker(operation);
});

// Navigate in picker
var pickerBreadcrumb = document.getElementById("dir-picker-breadcrumb");
if (pickerBreadcrumb) {
  pickerBreadcrumb.addEventListener("click", function (e) {
    var navBtn = e.target.closest(".js-picker-nav");
    if (!navBtn) return;
    loadPickerDirectory(navBtn.dataset.path);
  });
}

var pickerList = document.getElementById("dir-picker-list");
if (pickerList) {
  pickerList.addEventListener("click", function (e) {
    var dirEl = e.target.closest(".js-picker-dir");
    if (!dirEl) return;
    var name = dirEl.dataset.path;
    if (name) {
      var newPath = pickerPath === "/" ? "/" + name : pickerPath + "/" + name;
      loadPickerDirectory(newPath);
    }
  });
}

// Confirm move/copy
var pickerConfirmBtn = document.querySelector(".js-picker-confirm");
if (pickerConfirmBtn) {
  pickerConfirmBtn.addEventListener("click", async function () {
    var op = pickerOperation;
    if (!op || getSelectedCount() === 0) return;

    var repoId = getRepoId();
    var parentDir = getCurrentDir();

    // Prevent moving to the same directory (server would create duplicates)
    if (op === "move" && pickerPath === parentDir) {
      Toast.error(__t('ui.dest_same_as_source'));
      return;
    }

    closeDirPicker();

    try {
      var apiPath = op === "move"
        ? "/api/v2.1/repos/sync-batch-move-item/"
        : "/api/v2.1/repos/sync-batch-copy-item/";

      await apiFetch(apiPath, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          src_repo_id: repoId,
          src_parent_dir: parentDir,
          src_dirents: getSelectedPaths(),
          dst_repo_id: repoId,
          dst_parent_dir: pickerPath,
        }),
      });
      Toast.success(__t(op === "move" ? 'ui.moved_n' : 'ui.copied_n', { n: getSelectedCount() }));
      clearSelection();
      refreshFileList();
    } catch (err) {
      Toast.error(__t('ui.batch_op_failed', { op: op, msg: err.message }));
    }
  });
}

// Cancel picker
var pickerCancelBtn = document.querySelector(".js-picker-cancel");
if (pickerCancelBtn) {
  pickerCancelBtn.addEventListener("click", function () {
    closeDirPicker();
  });
}

// Click outside picker content to close
document.addEventListener("click", function (e) {
  var overlay = document.getElementById("dir-picker-overlay");
  if (!overlay || overlay.classList.contains("hidden")) return;
  if (e.target === overlay) closeDirPicker();
});

// Escape to close picker
document.addEventListener("keydown", function (e) {
  if (e.key !== "Escape") return;
  var overlay = document.getElementById("dir-picker-overlay");
  if (overlay && !overlay.classList.contains("hidden")) {
    closeDirPicker();
  }
});

// ─── Zip download (folder / batch) ──────────────────────────────────────
function zipDownload(repoId, parentDir, dirents) {
  var json = JSON.stringify({
    parent_dir: parentDir,
    dirents: dirents,
  });
  return fetch("/api/v2.1/repos/" + repoId + "/zip-task/", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-CSRFToken": getCookie("sfcsrftoken"),
    },
    body: json,
  })
    .then(function (r) {
      if (!r.ok) throw new Error("HTTP " + r.status);
      return r.json();
    })
    .then(function (data) {
      if (data.zip_token) {
        window.location.href = "/zip/" + data.zip_token;
      } else {
        throw new Error("No zip_token in response");
      }
    })
    .catch(function (err) {
      console.error("Zip download failed", err);
      Toast.error(__t('ui.download_failed', { msg: err.message }));
    });
}

// Single folder download button (.js-entry-download)
document.addEventListener("click", function (e) {
  var btn = e.target.closest(".js-entry-download");
  if (!btn) return;
  e.preventDefault();

  var row = btn.closest(".js-entry-row");
  if (!row) return;
  var repoId = row.dataset.repoId;
  var parentDir = getCurrentDir();
  var name = row.dataset.name;

  zipDownload(repoId, parentDir, [name]);
});

// Batch download selected items (.js-batch-download)
document.addEventListener("click", function (e) {
  var btn = e.target.closest(".js-batch-download");
  if (!btn) return;

  if (getSelectedCount() === 0) return;

  var repoId = getRepoId();
  var parentDir = getCurrentDir();

  zipDownload(repoId, parentDir, getSelectedPaths());
});

// ─── Batch reindex (.js-batch-reindex) ──────────────────────────────────
document.addEventListener("click", async function (e) {
  var btn = e.target.closest(".js-batch-reindex");
  if (!btn) return;

  var rows = document.querySelectorAll(".js-entry-row.selected");
  var files = [];
  for (var i = 0; i < rows.length; i++) {
    if (rows[i].dataset.type === "dir") continue;
    files.push({ repoId: rows[i].dataset.repoId, path: rows[i].dataset.path });
  }
  if (files.length === 0) {
    Toast.info(__t('ui.no_files_selected'));
    return;
  }

  btn.disabled = true;
  btn.textContent = "Indexing...";

  try {
    var indexedCount = 0;
    var skippedCount = 0;
    for (var j = 0; j < files.length; j++) {
      var resp = await apiFetch("/api2/repos/" + encodeURIComponent(files[j].repoId) + "/file/reindex/", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ p: files[j].path }),
      });
      var result = await resp.json();
      if (result.indexed) {
        indexedCount++;
      } else {
        skippedCount++;
      }
    }
    if (indexedCount > 0) {
      Toast.success(__t('ui.reindexed_n', { n: indexedCount }));
    }
    if (skippedCount > 0) {
      Toast.info(skippedCount + " file(s) skipped (unsupported type)");
    }
  } catch (e) {
    Toast.error(__t('ui.reindex_failed', { msg: e.message || e }));
  } finally {
    btn.disabled = false;
    btn.textContent = __t('ui.reindex_selected');
  }
});

// Right panel download — ZIP for directories
document.addEventListener("click", function (e) {
  var link = e.target.closest(".js-rp-download");
  if (!link) return;
  if (link.dataset.type !== "dir") return;
  e.preventDefault();
  var repoId = link.dataset.repoId;
  var name = link.dataset.name;
  if (!repoId || !name) return;
  var parentDir = link.dataset.path;
  if (parentDir.endsWith(name)) {
    parentDir = parentDir.slice(0, -name.length).replace(/\/+$/, "") || "/";
  }
  zipDownload(repoId, parentDir, [name]);
});
