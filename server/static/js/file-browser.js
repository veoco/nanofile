// Nanofile Web UI — File browser specific JS
(function () {
  "use strict";

  // ─── View mode toggle (list / grid / gallery) ──────────────────────────
  function setMode(m) {
    var listView = document.querySelector(".js-file-list-view");
    var gridView = document.querySelector(".js-file-grid-view");
    var galleryView = document.querySelector(".js-gallery-view");
    var sortBar = document.querySelector(".js-sort-bar");
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
    if (typeof window.syncSelectionView === "function") {
      window.syncSelectionView();
    }
    if (typeof window.syncPaginationBar === "function") {
      window.syncPaginationBar();
    }
  }

  window.setMode = setMode;

  // Returns the current view mode — used by main.js for partial reloads.
  window.getVisibleView = function () {
    var gv = document.querySelector(".js-gallery-view");
    if (gv && !gv.classList.contains("hidden")) return "gallery";
    var gridV = document.querySelector(".js-file-grid-view");
    if (gridV && !gridV.classList.contains("hidden")) return "grid";
    return "list";
  };

  // Event delegation on document so view toggle works after partial refresh
  document.addEventListener("click", function (e) {
    var btn = e.target.closest(".js-view-list");
    if (btn) { setMode("list"); return; }
    btn = e.target.closest(".js-view-grid");
    if (btn) { setMode("grid"); return; }
    btn = e.target.closest(".js-view-gallery");
    if (btn) { setMode("gallery"); }
  });

  // Initialize mode from localStorage on page load
  var mode = localStorage.getItem("fileViewMode") || "list";
  setMode(mode);

  // ─── Sort controls ──────────────────────────────────────────────────
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

  function initSortUI() {
    var sortBar = document.querySelector(".js-sort-bar");
    if (!sortBar) return;
    applySortUI(sortBar.dataset.sortField || "name", sortBar.dataset.sortOrder || "asc");
  }
  window.initSortUI = initSortUI;

  window.getSort = function () {
    var sortBar = document.querySelector(".js-sort-bar");
    if (sortBar) {
      return { sort: sortBar.dataset.sortField || "name", sort_order: sortBar.dataset.sortOrder || "asc" };
    }
    return { sort: localStorage.getItem("fileSortField") || "name", sort_order: localStorage.getItem("fileSortOrder") || "asc" };
  };

  function setSort(field) {
    var s = window.getSort();
    var order = field === s.sort ? (s.sort_order === "asc" ? "desc" : "asc") : "asc";
    localStorage.setItem("fileSortField", field);
    localStorage.setItem("fileSortOrder", order);
    applySortUI(field, order);
    if (typeof window.refreshFileList === "function") window.refreshFileList();
  }

  document.addEventListener("click", function (e) {
    var btn = e.target.closest(".js-sort-btn");
    if (btn) { setSort(btn.dataset.sort); return; }
  });

  // Initialize sort UI from server-rendered data attributes
  initSortUI();

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

  // ─── Right panel ─────────────────────────────────────────────────────
  window.openRightPanel = function (d) {
    // d = { name, type, size, sizeDisplay, mtime, mtimeDisplay, starred,
    //       extension, path, repoId, modifierEmail, thumbnailUrl, isPreviewable,
    //       downloadUrl, isVideo }

    var ph = document.querySelector(".js-rp-placeholder");
    var ct = document.querySelector(".js-rp-content");
    if (!ph || !ct) return;

    // Show content, hide placeholder
    ph.classList.add("hidden");
    ct.classList.remove("hidden");

    // ── Preview ──
    var thumbImg = ct.querySelector(".js-rp-thumb-img");
    var extBadge = ct.querySelector(".js-rp-ext-badge");
    var folderIcon = ct.querySelector(".js-rp-folder-icon");
    var videoIcon = ct.querySelector(".js-rp-video-icon");

    // Hide all preview variants first
    if (thumbImg) { thumbImg.classList.add("hidden"); thumbImg.src = ""; }
    if (extBadge) extBadge.classList.add("hidden");
    if (folderIcon) folderIcon.classList.add("hidden");
    if (videoIcon) videoIcon.classList.add("hidden");

    if (d.type === "dir") {
      if (folderIcon) folderIcon.classList.remove("hidden");
    } else if (d.thumbnailUrl) {
      if (thumbImg) { thumbImg.src = d.thumbnailUrl; thumbImg.classList.remove("hidden"); }
    } else if (d.isVideo && videoIcon) {
      if (videoIcon) videoIcon.classList.remove("hidden");
    } else if (d.extension && extBadge) {
      extBadge.textContent = d.extension;
      extBadge.classList.remove("hidden");
    } else if (extBadge) {
      extBadge.textContent = "?";
      extBadge.classList.remove("hidden");
    }

    // ── Basic Info ──
    setText(ct, ".js-rp-name", d.name || "");
    setText(ct, ".js-rp-type", humanType(d.type, d.extension));
    setText(ct, ".js-rp-size", d.sizeDisplay || "");

    // Show/hide size row (files only)
    var sizeRow = ct.querySelector(".js-rp-size-row");
    if (sizeRow) sizeRow.classList.toggle("hidden", d.type === "dir");

    setText(ct, ".js-rp-mtime", d.mtimeDisplay || "");

    // ── Starred ──
    var starBtn = ct.querySelector(".js-rp-starred");
    if (starBtn) {
      var isStarred = d.starred === true || d.starred === "true";
      starBtn.dataset.starred = isStarred ? "true" : "false";
      starBtn.dataset.repoId = d.repoId || "";
      starBtn.dataset.path = d.path || "";
      starBtn.setAttribute("data-toggle-star", "");
      var starIcon = ct.querySelector(".js-rp-star-icon");
      var starLabel = ct.querySelector(".js-rp-star-label");
      if (starIcon) {
        starIcon.setAttribute("fill", isStarred ? "currentColor" : "none");
      }
      if (starLabel) starLabel.textContent = isStarred ? "Starred" : "Not starred";
      starBtn.className =
        "inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs font-medium transition-colors " +
        (isStarred
          ? "text-amber-500 bg-amber-50 dark:bg-amber-900/20 hover:bg-amber-100 dark:hover:bg-amber-900/30"
          : "text-gray-400 hover:text-amber-500 hover:bg-amber-50 dark:hover:bg-amber-900/20");
    }

    // ── Details ──
    setText(ct, ".js-rp-path", d.path || "");
    setText(ct, ".js-rp-extension", d.extension || "");

    // Show/hide extension row (files only)
    var extRow = ct.querySelector(".js-rp-extension-row");
    if (extRow) extRow.classList.toggle("hidden", d.type === "dir" || !d.extension);

    // ── Actions ──
    // Download
    var downloadRow = ct.querySelector(".js-rp-download-row");
    if (downloadRow) downloadRow.classList.toggle("hidden", d.type === "dir");
    var downloadLink = ct.querySelector(".js-rp-download");
    if (downloadLink) {
      downloadLink.href = d.downloadUrl || "#";
      downloadLink.classList.toggle("pointer-events-none", !d.downloadUrl);
      downloadLink.classList.toggle("opacity-50", !d.downloadUrl);
    }

    // Delete
    var deleteBtn = ct.querySelector(".js-rp-delete-btn");
    if (deleteBtn) {
      deleteBtn.dataset.repoId = d.repoId || "";
      deleteBtn.dataset.path = d.path || "";
      deleteBtn.dataset.name = d.name || "";
      deleteBtn.dataset.type = d.type || "";
    }

    // Share
    var shareBtn = ct.querySelector(".js-rp-share");
    if (shareBtn) {
      shareBtn.dataset.repoId = d.repoId || "";
      shareBtn.dataset.path = d.path || "";
      shareBtn.dataset.type = d.type || "";
    }
  };

  // Reset right panel to placeholder state
  window.resetRightPanel = function () {
    var ph = document.querySelector(".js-rp-placeholder");
    var ct = document.querySelector(".js-rp-content");
    if (ph) ph.classList.remove("hidden");
    if (ct) ct.classList.add("hidden");
  };

  // ─── Helpers ─────────────────────────────────────────────────────────
  function setText(container, selector, val) {
    var el = container.querySelector(selector);
    if (el) el.textContent = val;
  }

  function humanType(type, ext) {
    if (type === "dir") return "Folder";
    if (!ext) return "File";
    var map = {
      "PNG": "PNG Image", "JPG": "JPEG Image", "JPEG": "JPEG Image",
      "GIF": "GIF Image", "WEBP": "WebP Image", "BMP": "BMP Image",
      "SVG": "SVG Image",
      "PDF": "PDF Document",
      "DOC": "Word Document", "DOCX": "Word Document",
      "XLS": "Excel Spreadsheet", "XLSX": "Excel Spreadsheet",
      "PPT": "PowerPoint", "PPTX": "PowerPoint",
      "TXT": "Text File", "MD": "Markdown File",
      "RS": "Rust Source", "PY": "Python Script", "JS": "JavaScript File",
      "TS": "TypeScript File", "GO": "Go Source", "JAVA": "Java Source",
      "C": "C Source", "CPP": "C++ Source", "H": "Header File",
      "RB": "Ruby Script", "PHP": "PHP Script", "SH": "Shell Script",
      "HTML": "HTML File", "CSS": "CSS File",
      "TOML": "TOML File", "JSON": "JSON File", "YAML": "YAML File", "YML": "YAML File",
      "CSV": "CSV File", "XML": "XML File", "SQL": "SQL File",
      "ZIP": "ZIP Archive", "TAR": "TAR Archive", "GZ": "GZip Archive",
      "BZ2": "BZip2 Archive", "7Z": "7-Zip Archive", "RAR": "RAR Archive",
      "MP4": "MP4 Video", "MOV": "MOV Video", "AVI": "AVI Video",
      "MKV": "MKV Video", "WEBM": "WebM Video", "WMV": "WMV Video",
      "MP3": "MP3 Audio", "FLAC": "FLAC Audio", "WAV": "WAV Audio",
      "ISO": "Disk Image"
    };
    return map[ext] || ext + " File";
  }

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
