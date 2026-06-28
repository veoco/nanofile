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
        if (q) window.location.href = "/search?q=" + encodeURIComponent(q);
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

})();
