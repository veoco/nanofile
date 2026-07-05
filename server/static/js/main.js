// Nanofile Web UI — main.js
(function () {
  "use strict";

  // ─── Toast notification system ────────────────────────────────────────
  var toastContainer = null;
  function initToast() {
    toastContainer = document.createElement("div");
    toastContainer.className =
      "fixed top-4 right-4 z-[9999] flex flex-col gap-2 pointer-events-none";
    toastContainer.setAttribute("aria-live", "polite");
    toastContainer.setAttribute("aria-relevant", "additions removals");
    document.body.appendChild(toastContainer);
  }

  function showToast(message, type, duration) {
    type = type || "success";
    duration = duration || 4000;
    if (!toastContainer) initToast();

    var colors = {
      success:
        "bg-green-50 border-green-200 text-green-800",
      error:
        "bg-red-50 border-red-200 text-red-800",
      info:
        "bg-brand-50 border-brand-200 text-brand-800",
    };

    var icons = {
      success:
        '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/>',
      error:
        '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-2.5L13.732 4c-.77-.833-1.964-.833-2.732 0L4.082 16.5c-.77.833.192 2.5 1.732 2.5z"/>',
      info:
        '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/>',
    };

    var el = document.createElement("div");
    el.className =
      "pointer-events-auto flex items-center gap-3 rounded-xl border px-4 py-3 shadow-lg animate-slide-in " +
      (colors[type] || colors.success);
    el.innerHTML =
      '<svg class="h-5 w-5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">' +
      (icons[type] || icons.success) +
      '</svg><p class="text-sm font-medium flex-1">' +
      escapeHtml(message) +
      '</p><button class="flex-shrink-0 rounded-md p-1 opacity-60 hover:opacity-100 transition-opacity" onclick="this.parentElement.classList.add(\'animate-slide-out\');setTimeout(function(){this.parentElement.remove()}.bind(this),250)">' +
      '<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>' +
      "</button>";

    toastContainer.appendChild(el);

    setTimeout(function () {
      el.classList.add("animate-slide-out");
      setTimeout(function () { if (el.parentNode) el.remove(); }, 250);
    }, duration);
  }

  // Expose globally for inline scripts in templates
  window.Toast = { show: showToast, success: function(m) { showToast(m, "success"); }, error: function(m) { showToast(m, "error"); }, info: function(m) { showToast(m, "info"); } };

  // ─── Loading bar ─────────────────────────────────────────────────────
  var loadingBar = document.getElementById("loading-bar");
  window.showLoading = function () {
    if (loadingBar) loadingBar.classList.remove("hidden");
  };
  window.hideLoading = function () {
    if (loadingBar) loadingBar.classList.add("hidden");
  };

  // ─── Mobile left panel toggle ──────────────────────────────────────
  const menuToggle = document.querySelector(".js-mobile-menu-toggle");
  const leftPanel = document.querySelector(".js-left-panel");

  function toggleMobilePanel() {
    if (!leftPanel) return;
    if (leftPanel.classList.contains("hidden")) {
      leftPanel.classList.remove("hidden");
      leftPanel.classList.add("flex");
      leftPanel.style.width = "var(--left-panel-width, 240px)";
    } else {
      leftPanel.classList.add("hidden");
      leftPanel.classList.remove("flex");
      leftPanel.style.width = "0";
    }
  }

  if (menuToggle) {
    menuToggle.addEventListener("click", function (e) {
      e.stopPropagation();
      toggleMobilePanel();
    });
  }
  // ─── User menu dropdown (Sign out only) ─────────────────────────
  const userMenu = document.querySelector(".js-user-menu");
  const userButton = document.querySelector(".js-user-menu-button");
  if (userMenu && userButton) {
    userButton.addEventListener("click", function (e) {
      e.stopPropagation();
      let dropdown = userMenu.querySelector(".js-user-menu-dropdown");
      if (dropdown) { dropdown.remove(); return; }
      dropdown = document.createElement("div");
      dropdown.className =
        "js-user-menu-dropdown absolute right-0 z-50 mt-2 w-36 origin-top-right rounded-xl bg-white dark:bg-surface-800 py-1 shadow-lg ring-1 ring-black/5 dark:ring-white/10 focus:outline-none";

      var a = document.createElement("a");
      a.href = "/accounts/logout/";
      a.className = "block px-4 py-2 text-sm text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-surface-700";
      a.textContent = "Sign out";
      dropdown.appendChild(a);

      userMenu.appendChild(dropdown);

      document.addEventListener(
        "click",
        function closeMenu(ev) {
          if (!userMenu.contains(ev.target)) {
            dropdown.remove();
            document.removeEventListener("click", closeMenu);
          }
        },
        { once: true }
      );
    });
  }

  // ─── Dark mode toggle ────────────────────────────────────────────────
  const darkToggle = document.querySelector(".js-dark-toggle");
  if (darkToggle) {
    darkToggle.addEventListener("click", function () {
      document.documentElement.classList.toggle("dark");
      localStorage.setItem(
        "darkMode",
        document.documentElement.classList.contains("dark")
      );
    });
    if (localStorage.getItem("darkMode") === "true") {
      document.documentElement.classList.add("dark");
    }
  }

  // ─── Quick search ────────────────────────────────────────────────────
  var searchInput = document.querySelector(".js-quick-search");
  if (searchInput) {
    searchInput.addEventListener("keydown", function (e) {
      if (e.key === "Enter") {
        var q = searchInput.value.trim();
        if (q) window.location.href = "/search/?q=" + encodeURIComponent(q);
      }
    });
  }

  // ─── Keyboard shortcuts ──────────────────────────────────────────────
  var searchFocused = false;
  document.addEventListener("keydown", function (e) {
    var tag = (e.target && e.target.tagName) || "";
    var isInput = tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";

    // Close sidebar on Escape
    if (e.key === "Escape") {
      closeSidebar();
      return;
    }

    // / key to focus search
    if (e.key === "/" && !isInput) {
      e.preventDefault();
      if (searchInput) { searchInput.focus(); searchInput.select(); }
      return;
    }

    // g then another key for navigation (only when not in an input)
    if (!isInput) {
      if (e.key === "g" && !searchFocused) {
        searchFocused = true;
        var navTimer = setTimeout(function () { searchFocused = false; }, 1000);
        document.addEventListener(
          "keydown",
          function navHandler(ev) {
            if (ev.key === "l") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/libraries/"; }
            else if (ev.key === "s") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/starred/"; }
            else if (ev.key === "t") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/trash/"; }
            else if (ev.key === "a") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/activities/"; }
            else if (ev.key === "p") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/profile/"; }
            else if (ev.key === "Escape") { clearTimeout(navTimer); searchFocused = false; }
            document.removeEventListener("keydown", navHandler);
          },
          { once: true }
        );
      }
    }
  });

  // ─── Star toggle (event delegation) ──────────────────────────────────
  document.addEventListener("click", async function (e) {
    const btn = e.target.closest("[data-toggle-star]");
    if (!btn) return;

    const repoId = btn.dataset.repoId;
    const path = btn.dataset.path;
    const currentlyStarred = btn.dataset.starred === "true";
    const csrfToken = getCookie("sfcsrftoken");
    if (!csrfToken) {
      window.location.href = "/accounts/login/";
      return;
    }

    btn.disabled = true;

    try {
      if (currentlyStarred) {
        const url =
          "/api/v2.1/starred-items/?repo_id=" +
          encodeURIComponent(repoId) +
          "&path=" +
          encodeURIComponent(path);
        const res = await fetch(url, {
          method: "DELETE",
          headers: { "X-CSRFToken": csrfToken },
        });
        if (res.ok) {
          btn.classList.remove("text-yellow-400", "text-amber-400");
          btn.classList.add("text-gray-300", "hover:text-yellow-400");
          if (btn.classList.contains("dark\\:text-gray-600")) {
            btn.classList.add("dark:text-gray-600");
          }
          btn.querySelector("svg").setAttribute("fill", "none");
          btn.title = "Star";
          btn.dataset.starred = "false";
        }
      } else {
        const res = await fetch("/api/v2.1/starred-items/", {
          method: "POST",
          headers: {
            "X-CSRFToken": csrfToken,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ repo_id: repoId, path: path }),
        });
        if (res.ok) {
          btn.classList.remove("text-gray-300", "hover:text-yellow-400");
          if (btn.classList.contains("dark\\:text-gray-600")) {
            btn.classList.remove("dark:text-gray-600");
          }
          btn.classList.add("text-amber-400");
          btn.querySelector("svg").setAttribute("fill", "currentColor");
          btn.title = "Unstar";
          btn.dataset.starred = "true";
        }
      }
    } catch (ignored) {
      // Ignore network errors silently
    } finally {
      btn.disabled = false;
    }
  });

  // ─── Cookie helper ───────────────────────────────────────────────────
  function getCookie(name) {
    const match = document.cookie.match(
      "(^|;)\\s*" + name + "\\s*=\\s*([^;]+)"
    );
    return match ? match.pop() : "";
  }

  function escapeHtml(str) {
    var div = document.createElement("div");
    div.appendChild(document.createTextNode(str));
    return div.innerHTML;
  }

  // ─── Authenticated fetch helper ─────────────────────────────────────
  window.apiFetch = async function (url, options) {
    options = options || {};
    var headers = options.headers || {};
    if (!headers["X-CSRFToken"]) {
      headers["X-CSRFToken"] = getCookie("sfcsrftoken");
    }
    if (
      options.body &&
      !(options.body instanceof FormData) &&
      typeof options.body === "string" &&
      !headers["Content-Type"]
    ) {
      headers["Content-Type"] = "application/json;charset=utf-8";
    }
    options.headers = headers;

    var res = await fetch(url, options);
    if (!res.ok) {
      var text = await res.text().catch(function () { return res.statusText; });
      throw new Error(text || res.statusText);
    }
    return res;
  };

  // ─── Custom confirm dialog ───────────────────────────────────────────
  var confirmOverlay = null;
  var confirmResolve = null;

  function initConfirmDialog() {
    confirmOverlay = document.createElement("div");
    confirmOverlay.className =
      "hidden fixed inset-0 z-[100] flex items-center justify-center bg-black/30";
    confirmOverlay.setAttribute("role", "alertdialog");
    confirmOverlay.setAttribute("aria-modal", "true");
    confirmOverlay.innerHTML =
      '<div class="bg-white dark:bg-surface-800 rounded-xl shadow-xl p-6 w-full max-w-sm mx-4" onclick="event.stopPropagation()">' +
      '<h3 class="text-base font-semibold text-gray-900 dark:text-gray-100 mb-1 js-confirm-title"></h3>' +
      '<p class="text-sm text-gray-500 dark:text-gray-400 mb-4 js-confirm-message"></p>' +
      '<div class="flex justify-end gap-2">' +
      '<button class="js-confirm-cancel rounded-lg bg-white dark:bg-surface-700 px-4 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 border border-gray-300 dark:border-gray-600 hover:bg-gray-50 dark:hover:bg-surface-600 transition-colors">Cancel</button>' +
      '<button class="js-confirm-ok rounded-lg px-4 py-2 text-sm font-medium text-white transition-colors"></button>' +
      "</div></div>";
    document.body.appendChild(confirmOverlay);

    confirmOverlay.addEventListener("click", function (e) {
      if (e.target === confirmOverlay) hideConfirm(false);
    });

    confirmOverlay.querySelector(".js-confirm-cancel").addEventListener("click", function () {
      hideConfirm(false);
    });

    document.addEventListener("keydown", function confirmEsc(e) {
      if (e.key === "Escape" && confirmOverlay && !confirmOverlay.classList.contains("hidden")) {
        hideConfirm(false);
      }
    });
  }

  function hideConfirm(result) {
    if (confirmOverlay) confirmOverlay.classList.add("hidden");
    if (confirmResolve) { confirmResolve(result); confirmResolve = null; }
  }

  function showConfirmDialog(title, message, opts) {
    opts = opts || {};
    if (!confirmOverlay) initConfirmDialog();

    confirmOverlay.querySelector(".js-confirm-title").textContent = title;
    confirmOverlay.querySelector(".js-confirm-message").textContent = message;

    var okBtn = confirmOverlay.querySelector(".js-confirm-ok");
    okBtn.textContent = opts.confirmText || "Delete";
    okBtn.className =
      "js-confirm-ok rounded-lg px-4 py-2 text-sm font-medium text-white transition-colors " +
      (opts.variant === "danger"
        ? "bg-red-600 hover:bg-red-700"
        : "bg-brand-500 hover:bg-brand-600");

    // Remove old listener by cloning
    var newOk = okBtn.cloneNode(true);
    okBtn.parentNode.replaceChild(newOk, okBtn);

    confirmOverlay.classList.remove("hidden");
    // Focus the cancel button by default
    setTimeout(function () {
      confirmOverlay.querySelector(".js-confirm-cancel").focus();
    }, 100);

    return new Promise(function (resolve) {
      confirmResolve = resolve;
      newOk.addEventListener("click", function () { hideConfirm(true); });
    });
  }

  window.ConfirmDialog = {
    confirm: function (title, message, opts) { return showConfirmDialog(title, message, opts); },
  };

  // ─── Delete file/dir via API ─────────────────────────────────────────
  document.addEventListener("click", async function (e) {
    const btn = e.target.closest(".js-delete-btn");
    if (!btn) return;

    var repoId = btn.dataset.repoId;
    var path = btn.dataset.path;
    var name = btn.dataset.name;
    var entryType = btn.dataset.type;

    var confirmed = await showConfirmDialog(
      "Delete",
      'Delete "' + name + '"? This cannot be undone.',
      { confirmText: "Delete", variant: "danger" }
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
        if (window.refreshFileList) window.refreshFileList();
        else window.location.reload();
      } else {
        var text = await res.text().catch(function () { return res.statusText; });
        window.Toast.error("Delete failed: " + text);
      }
    } catch (err) {
      window.Toast.error("Delete failed: " + err.message);
    }
  });

  // ─── Share dialog elements ──────────────────────────────────────────
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

  // ─── Share button — open dialog ─────────────────────────────────────
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
    shareConfirmBtn.textContent = "Create";
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

  // ─── Share dialog event listeners (only on pages with the dialog) ────
  if (shareDialog) {
    shareCancelBtn.addEventListener("click", function () {
      shareDialog.classList.add("hidden");
    });
    shareDialog.addEventListener("click", function (e) {
      if (e.target === shareDialog) shareDialog.classList.add("hidden");
    });

    // ─── Share delete — delete link via API ────────────────────────────
    shareDeleteBtn.addEventListener("click", async function () {
      if (!shareCurrentToken) return;
      if (!confirm("Delete this share link?")) return;

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
          shareConfirmBtn.textContent = "Create";
          shareCancelBtn.classList.remove("hidden");
          shareDialogError.textContent = "";
        } else {
          var text = await resp.text().catch(function () { return ""; });
          shareDialogError.textContent = text || "Failed to delete share link";
          shareDialogError.classList.remove("hidden");
        }
      } catch (err) {
        shareDialogError.textContent = err.message;
        shareDialogError.classList.remove("hidden");
      } finally {
        shareDeleteBtn.disabled = false;
      }
    });

    // ─── Share confirm — create link via API ────────────────────────────
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
          shareConfirmBtn.textContent = "Create";
        }
      }
    });
  }

  // ─── Trash restore (via API) ────────────────────────────────────────
  document.addEventListener("submit", async function (e) {
    const form = e.target.closest(".js-restore-form");
    if (!form) return;
    e.preventDefault();

    const repoId = form.querySelector('[name="repo_id"]').value;
    const commitId = form.querySelector('[name="commit_id"]').value;
    const path = form.querySelector('[name="path"]').value;
    const objName = form.dataset.objName || "";
    const repoName = form.dataset.repoName || "";

    var confirmed = await showConfirmDialog(
      "Restore",
      'Restore "' + objName + '" from ' + repoName + "?",
      { confirmText: "Restore", variant: "primary" }
    );
    if (!confirmed) return;

    // Build request body: { commit_id: [path] }
    var body = {};
    body[commitId] = [path];

    var csrfToken = (function(){var m=document.cookie.match(/(^|;)\s*sfcsrftoken\s*=\s*([^;]+)/);return m?m[2]:'';})();
    try {
      var resp = await fetch('/api/v2.1/repos/' + encodeURIComponent(repoId) + '/trash2/revert/', {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
          'Content-Type': 'application/json',
          'X-CSRFToken': csrfToken,
        },
        body: JSON.stringify(body),
      });
      if (resp.ok) {
        window.location.reload();
      } else {
        window.Toast.error("Restore failed");
      }
    } catch (err) {
      window.Toast.error("Restore failed: " + err.message);
    }
  });

  // ─── Rename dialog ──────────────────────────────────────────────────
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

        var res = await window.apiFetch(apiPath, {
          method: "POST",
          body: formBody.toString(),
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
        });
        window.Toast.success('Renamed to "' + newName + '"');
        if (window.refreshFileList) window.refreshFileList();
        else window.location.reload();
      } catch (err) {
        window.Toast.error("Rename failed: " + err.message);
      }
    });
  }

  // ─── Batch operations: selection state ──────────────────────────────────
  var selectedPaths = new Set();
  var anchorPath = null;       // Anchor for Shift+click range selection
  var touchSelectMode = false; // Touch multi-select mode (long-press activated)
  var suppressClick = false;   // Suppress synthetic click after long-press
  var pickerOperation = null;  // "move" or "copy"
  var pickerPath = "/";

  function getCurrentDir() {
    var input = document.querySelector('[name="current_dir"]');
    if (input && input.value) return input.value;
    var m = window.location.pathname.match(/\/files\/(.*)/);
    return m ? "/" + m[1] : "/";
  }

  function getRepoId() {
    var meta = document.querySelector('meta[name="repo-id"]');
    return meta ? meta.content : "";
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
      selBtn.textContent = selectedPaths.size === totalRows ? "Deselect All" : "Select All";
    }
  }

  function clearSelection() {
    touchSelectMode = false;
    selectedPaths.clear();
    document.querySelectorAll(".js-entry-row.selected").forEach(function (row) {
      row.classList.remove("selected");
    });
    updateSelectionBar();
    if (typeof window.resetRightPanel === "function") window.resetRightPanel();
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

  function getSelectedItems() {
    var items = [];
    document.querySelectorAll(".js-entry-row.selected").forEach(function (r) {
      items.push({ name: r.dataset.name, type: r.dataset.type });
    });
    return items;
  }

  // Update right panel based on current selection state
  function updateSelectionPanel() {
    var count = selectedPaths.size;
    if (count === 0) {
      if (typeof window.resetRightPanel === "function") window.resetRightPanel();
      return;
    }
    if (count === 1) {
      var selRow = document.querySelector(".js-entry-row.selected");
      if (selRow && typeof window.openRightPanel === "function") {
        var dlUrl = selRow.dataset.type !== "dir"
          ? "/libraries/" + selRow.dataset.repoId + "/files/" + selRow.dataset.path + "?dl=1"
          : "";
        window.openRightPanel({
          name: selRow.dataset.name,
          type: selRow.dataset.type,
          size: selRow.dataset.size,
          sizeDisplay: selRow.dataset.sizeDisplay,
          mtime: selRow.dataset.mtime,
          mtimeDisplay: selRow.dataset.mtimeDisplay,
          starred: selRow.dataset.starred === "true",
          extension: selRow.dataset.extension,
          path: selRow.dataset.path,
          repoId: selRow.dataset.repoId,
          modifierEmail: selRow.dataset.modifierEmail,
          thumbnailUrl: selRow.dataset.thumbnailUrl,
          isPreviewable: selRow.dataset.isPreviewable === "true",
          downloadUrl: dlUrl,
        });
      }
      return;
    }
    // Multiple items selected
    if (typeof window.openMultiSelectPanel === "function") {
      window.openMultiSelectPanel(getSelectedItems());
    }
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
      if (typeof window.openMultiSelectPanel === "function") {
        window.openMultiSelectPanel(getSelectedItems());
      }
    }
  });

  document.addEventListener("click", function (e) {
    if (e.target.closest(".js-deselect-all")) {
      touchSelectMode = false;
      clearSelection();
    }
  });

  // ─── Touch selection support (long-press multi-select) ──────────────
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
  window.syncSelectionView = function () {
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
  };

  // ─── Load more (pagination) ─────────────────────────────────────────────
  function getVisibleViewContainer() {
    var listView = document.querySelector(".js-file-list-view");
    var gridView = document.querySelector(".js-file-grid-view");
    var galleryView = document.querySelector(".js-gallery-view");
    if (galleryView && !galleryView.classList.contains("hidden")) return galleryView;
    if (gridView && !gridView.classList.contains("hidden")) return gridView;
    return listView;
  }

  // Sync load-more bar state to the currently visible view
  window.syncPaginationBar = function () {
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
  };

  window.loadMoreEntries = async function () {
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

    var view = (typeof window.getVisibleView === "function") ? window.getVisibleView() : "list";
    var nextPage = page + 1;
    var sep = window.location.pathname.indexOf("?") !== -1 ? "&" : "?";
    var url = window.location.pathname + sep + "partial=1&view=" + view + "&page=" + nextPage;
    // Gallery loads more always use mtime-desc sort so groups remain reverse-chronological
    if (view === "gallery") {
      url += "&sort=mtime&sort_order=desc";
    } else {
      var sort = (typeof window.getSort === "function") ? window.getSort() : null;
      if (sort) url += '&sort=' + sort.sort + '&sort_order=' + sort.sort_order;
    }

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
  };

  // Load more button click handler
  document.addEventListener("click", function (e) {
    if (e.target.closest(".js-load-more-btn")) {
      window.loadMoreEntries();
    }
  });

  // ─── Infinite scroll (Phase 3) ───────────────────────────────────────────
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
        window.loadMoreEntries();
      }
    }, 200);
  }

  // Reusable — also called after DOM refresh so the observer reconnects
  var fileListObserver = null;
  window.initInfiniteScroll = function () {
    if (fileListObserver) { fileListObserver.disconnect(); fileListObserver = null; }
    var loadMoreBar = document.querySelector(".js-load-more-bar");
    if (loadMoreBar && "IntersectionObserver" in window) {
      fileListObserver = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          if (entry.isIntersecting) {
            window.loadMoreEntries();
          }
        });
      }, { rootMargin: "300px" });
      fileListObserver.observe(loadMoreBar);
    }
  };
  // Fallback scroll listener on <main> (runs once; <main> is not replaced on refresh)
  if (!("IntersectionObserver" in window)) {
    var mainEl = document.querySelector("main");
    if (mainEl) {
      mainEl.addEventListener("scroll", onFileListScroll, { passive: true });
    }
  }
  window.initInfiniteScroll();

  // ─── Batch delete ───────────────────────────────────────────────────────
  document.addEventListener("click", async function (e) {
    var btn = e.target.closest(".js-batch-delete");
    if (!btn) return;
    if (selectedPaths.size === 0) return;

    var confirmed = await showConfirmDialog(
      "Delete",
      "Delete " + selectedPaths.size + " item(s)? This cannot be undone.",
      { confirmText: "Delete", variant: "danger" }
    );
    if (!confirmed) return;

    var repoId = getRepoId();
    if (!repoId) { Toast.error("Cannot determine repo"); return; }
    var parentDir = getCurrentDir();
    if (!parentDir) { Toast.error("Cannot determine parent directory"); return; }

    try {
      await window.apiFetch("/api/v2.1/repos/batch-delete-item/", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          repo_id: repoId,
          parent_dir: parentDir,
          dirents: Array.from(selectedPaths),
        }),
      });
      Toast.success("Deleted " + selectedPaths.size + " item(s)");
      clearSelection();
      if (window.refreshFileList) window.refreshFileList();
      else window.location.reload();
    } catch (err) {
      Toast.error("Batch delete failed: " + err.message);
    }
  });

  // ─── Directory picker (for batch move/copy) ────────────────────────────
  function openDirPicker(operation) {
    pickerOperation = operation;
    pickerPath = getCurrentDir();

    var titleEl = document.getElementById("dir-picker-title");
    if (titleEl) {
      titleEl.textContent = (operation === "move" ? "Move" : "Copy") + " " + selectedPaths.size + " Item(s)";
    }

    var confirmBtn = document.querySelector(".js-picker-confirm");
    if (confirmBtn) {
      confirmBtn.textContent = operation === "move" ? "Move Here" : "Copy Here";
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
    if (selectedPaths.size === 0) return;
    var operation = btn.classList.contains("js-batch-move") ? "move" : "copy";
    openDirPicker(operation);
  });

  // Navigate in picker (direct listeners inside stopPropagation boundary)
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

  // Confirm move/copy (direct listener, inside stopPropagation boundary)
  var pickerConfirmBtn = document.querySelector(".js-picker-confirm");
  if (pickerConfirmBtn) {
    pickerConfirmBtn.addEventListener("click", async function () {
      var op = pickerOperation;
      if (!op || selectedPaths.size === 0) return;

      var repoId = getRepoId();
      var parentDir = getCurrentDir();

      // Prevent moving to the same directory (server would create duplicates)
      if (op === "move" && pickerPath === parentDir) {
        Toast.error("Destination is the same as source");
        return;
      }

      closeDirPicker();

      try {
        var apiPath = op === "move"
          ? "/api/v2.1/repos/sync-batch-move-item/"
          : "/api/v2.1/repos/sync-batch-copy-item/";

        await window.apiFetch(apiPath, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            src_repo_id: repoId,
            src_parent_dir: parentDir,
            src_dirents: Array.from(selectedPaths),
            dst_repo_id: repoId,
            dst_parent_dir: pickerPath,
          }),
        });
        Toast.success((op === "move" ? "Moved" : "Copied") + " " + selectedPaths.size + " item(s)");
        clearSelection();
        if (window.refreshFileList) window.refreshFileList();
        else window.location.reload();
      } catch (err) {
        Toast.error("Batch " + op + " failed: " + err.message);
      }
    });
  }

  // Cancel picker (direct listener, inside stopPropagation boundary)
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
    // If the click is on the overlay background (not the inner card), close
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

  // ─── Zip download (folder / batch) ───────────────────────────────
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
        if (typeof Toast !== "undefined") {
          Toast.error("Download failed: " + err.message);
        }
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
    var parentDir = (function () {
      var input = document.querySelector('[name="current_dir"]');
      if (input && input.value) return input.value;
      var m = window.location.pathname.match(/\/files\/(.*)/);
      return m ? "/" + m[1] : "/";
    })();
    var name = row.dataset.name;

    zipDownload(repoId, parentDir, [name]);
  });

  // Batch download selected items (.js-batch-download)
  document.addEventListener("click", function (e) {
    var btn = e.target.closest(".js-batch-download");
    if (!btn) return;

    if (selectedPaths.size === 0) return;

    var repoId = (function () {
      var meta = document.querySelector('meta[name="repo-id"]');
      return meta ? meta.content : "";
    })();
    var parentDir = (function () {
      var input = document.querySelector('[name="current_dir"]');
      if (input && input.value) return input.value;
      var m = window.location.pathname.match(/\/files\/(.*)/);
      return m ? "/" + m[1] : "/";
    })();

    zipDownload(repoId, parentDir, Array.from(selectedPaths));
  });

  // ─── Response time display ────────────────────────────────────
  var respTimeEl = document.getElementById("resp-time");
  if (respTimeEl) {
    window.addEventListener("load", function () {
      var loadTime = performance.now();
      var display = loadTime >= 2000
        ? (loadTime / 1000).toFixed(1) + "s"
        : Math.round(loadTime) + "ms";
      respTimeEl.textContent = display;
      respTimeEl.classList.remove("opacity-0");
    });
  }

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

})();
