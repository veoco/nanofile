// Nanofile Web UI — File browser specific JS
(function () {
  "use strict";

  // ─── View mode toggle (list / grid) ──────────────────────────────────
  var listView = document.querySelector(".js-file-list-view");
  var gridView = document.querySelector(".js-file-grid-view");
  var btnList = document.querySelector(".js-view-list");
  var btnGrid = document.querySelector(".js-view-grid");

  if (listView && gridView && btnList && btnGrid) {
    var mode = localStorage.getItem("fileViewMode") || "list";

    function setMode(m) {
      if (m === "grid") {
        listView.classList.add("hidden");
        gridView.classList.remove("hidden");
        btnList.classList.remove("text-brand-500");
        btnList.classList.add("text-gray-400");
        btnGrid.classList.remove("text-gray-400");
        btnGrid.classList.add("text-brand-500");
      } else {
        listView.classList.remove("hidden");
        gridView.classList.add("hidden");
        btnGrid.classList.remove("text-brand-500");
        btnGrid.classList.add("text-gray-400");
        btnList.classList.remove("text-gray-400");
        btnList.classList.add("text-brand-500");
      }
      localStorage.setItem("fileViewMode", m);
    }

    btnList.addEventListener("click", function () { setMode("list"); });
    btnGrid.addEventListener("click", function () { setMode("grid"); });
    setMode(mode);
  }

  // ─── Skeleton loading ────────────────────────────────────────────────
  var skeleton = document.querySelector(".js-skeleton");
  var fileListContainer = document.querySelector(".file-list-container");
  window.showFileSkeleton = function () {
    if (skeleton) skeleton.classList.remove("hidden");
    if (fileListContainer) {
      var list = fileListContainer.querySelector(".js-file-list-view");
      if (list) list.classList.add("hidden");
    }
  };
  window.hideFileSkeleton = function () {
    if (skeleton) skeleton.classList.add("hidden");
    if (fileListContainer) {
      var list = fileListContainer.querySelector(".js-file-list-view");
      if (list) list.classList.remove("hidden");
    }
  };

  // ─── Right panel toggle ─────────────────────────────────────────────
  var rightPanel = document.querySelector(".js-right-panel");
  var rightToggle = document.querySelector(".js-right-panel-toggle");
  function setRightPanel(visible) {
    if (!rightPanel) return;
    if (visible) {
      rightPanel.style.width = rightPanel.dataset.expandedWidth || "300px";
      rightPanel.style.overflow = "auto";
    } else {
      rightPanel.style.width = "0";
      rightPanel.style.overflow = "hidden";
    }
  }
  if (rightToggle) {
    rightToggle.addEventListener("click", function () { setRightPanel(false); });
  }
  window.openRightPanel = function (fileData) {
    setRightPanel(true);
    var titleEl = document.querySelector(".js-right-panel-title");
    var contentEl = document.querySelector(".js-right-panel-content");
    if (titleEl) titleEl.textContent = fileData.name || "Details";
    if (contentEl) {
      contentEl.innerHTML =
        '<div class="text-left space-y-3">' +
        '<p class="text-sm font-medium text-gray-900 dark:text-gray-100">' + escapeHtml(fileData.name) + "</p>" +
        (fileData.size ? '<p class="text-xs text-gray-500 dark:text-gray-400">Size: ' + escapeHtml(fileData.size) + "</p>" : "") +
        (fileData.mtime ? '<p class="text-xs text-gray-500 dark:text-gray-400">Modified: ' + escapeHtml(fileData.mtime) + "</p>" : "") +
        (fileData.downloadUrl ? '<a href="' + escapeHtml(fileData.downloadUrl) + '" class="inline-block mt-2 rounded-lg bg-brand-500 px-3 py-1.5 text-xs font-medium text-white hover:bg-brand-600">Download</a>' : "") +
        "</div>";
    }
  };

  // ─── Repo filter ────────────────────────────────────────────────────
  var repoFilter = document.querySelector(".js-repo-filter");
  if (repoFilter) {
    repoFilter.addEventListener("input", function () {
      var q = repoFilter.value.toLowerCase();
      var items = document.querySelectorAll(".js-repo-item");
      for (var i = 0; i < items.length; i++) {
        var name = (items[i].textContent || "").toLowerCase();
        items[i].style.display = name.indexOf(q) > -1 ? "" : "none";
      }
    });
  }

  // ─── New Library dialog ────────────────────────────────────────────
  window.showQuickCreate = function () {
    var overlay = document.getElementById("quick-create-overlay");
    if (!overlay) return;
    overlay.classList.remove("hidden");
    var input = document.getElementById("quick-create-input");
    if (input) { input.value = ""; setTimeout(function () { input.focus(); }, 100); }
  };
  window.hideQuickCreate = function () {
    var overlay = document.getElementById("quick-create-overlay");
    if (overlay) overlay.classList.add("hidden");
  };
  window.submitQuickCreate = function () {
    var input = document.getElementById("quick-create-input");
    var name = input ? input.value.trim() : "";
    if (!name) return false;
    var csrfToken = getCookie("sfcsrftoken");
    if (!csrfToken) { window.location.href = "/accounts/login/"; return false; }
    fetch("/api2/repos/", {
      method: "POST",
      headers: {
        "X-CSRFToken": csrfToken,
        "Content-Type": "application/json;charset=utf-8",
      },
      body: JSON.stringify({ name: name }),
    })
      .then(function (r) {
        if (r.ok) { window.location.reload(); }
        else { r.json().then(function (e) { window.Toast && Toast.error(e.error_msg || "Failed"); }); }
      })
      .catch(function () { window.Toast && Toast.error("Network error"); });
    hideQuickCreate();
    return false;
  };

  // ─── Helpers ──────────────────────────────────────────────────────────
  function escapeHtml(str) {
    var div = document.createElement("div");
    div.appendChild(document.createTextNode(str));
    return div.innerHTML;
  }

  function getCookie(name) {
    var match = document.cookie.match("(^|;)\\s*" + name + "\\s*=\\s*([^;]+)");
    return match ? match.pop() : "";
  }

})();
