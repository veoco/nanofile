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
    // d = { name, type, starred, extension, path, repoId, modifierEmail,
    //       thumbnailUrl, thumbnailUrlLarge, isPreviewable, downloadUrl, isVideo }

    var ph = document.querySelector(".js-rp-placeholder");
    var ct = document.querySelector(".js-rp-content");
    var mc = document.querySelector(".js-rp-multi-content");
    if (!ph || !ct) return;

    // Show content, hide placeholder and multi-select panel
    ph.classList.add("hidden");
    ct.classList.remove("hidden");
    if (mc) mc.classList.add("hidden");

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
    } else if (d.thumbnailUrlLarge || d.thumbnailUrl) {
      if (thumbImg) { thumbImg.src = d.thumbnailUrlLarge || d.thumbnailUrl; thumbImg.classList.remove("hidden"); }
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

    // ── Actions ──
    // Download
    var downloadRow = ct.querySelector(".js-rp-download-row");
    if (downloadRow) downloadRow.classList.remove("hidden");
    var downloadLink = ct.querySelector(".js-rp-download");
    if (downloadLink) {
      downloadLink.href = d.type === "dir" ? "#" : (d.downloadUrl || "#");
      downloadLink.classList.remove("pointer-events-none", "opacity-50");
      downloadLink.dataset.repoId = d.repoId || "";
      downloadLink.dataset.path = d.path || "";
      downloadLink.dataset.name = d.name || "";
      downloadLink.dataset.type = d.type || "";
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

    // ── Share Links (fetch existing links for this file) ──
    var shareSection = ct.querySelector(".js-rp-share-links-section");
    var shareList = ct.querySelector(".js-rp-share-links-list");
    var noLinks = ct.querySelector(".js-rp-no-share-links");
    if (shareSection && shareList && noLinks) {
      shareSection.classList.add("hidden");
      noLinks.classList.add("hidden");
      if (d.repoId && d.path) {
        fetch("/api/v2.1/share-links/?repo_id=" + encodeURIComponent(d.repoId) + "&path=" + encodeURIComponent(d.path))
          .then(function (r) { return r.json(); })
          .then(function (data) {
            var links = data.share_link_list || [];
            shareList.innerHTML = "";
            if (links.length === 0) {
              noLinks.classList.remove("hidden");
            } else {
              links.forEach(function (link) {
                var div = document.createElement("div");
                div.className = "flex items-center justify-between py-0.5";
                div.innerHTML =
                  '<a href="' + escapeHtml(link.link || "") + '" target="_blank" class="text-xs text-brand-500 hover:text-brand-600 truncate block">' +
                    escapeHtml(link.token || "") +
                  '</a>' +
                  '<span class="text-xs text-gray-400 flex-shrink-0 ml-2">' + (link.view_cnt || 0) + ' views</span>';
                shareList.appendChild(div);
              });
            }
            shareSection.classList.remove("hidden");
          })
          .catch(function () { /* ignore */ });
      }
    }

    // Upload links (directories only)
    var ulSection = ct.querySelector(".js-rp-upload-links-section");
    var ulList = ct.querySelector(".js-rp-upload-links-list");
    var noUl = ct.querySelector(".js-rp-no-upload-links");
    if (ulSection && ulList && noUl) {
      ulSection.classList.add("hidden");
      noUl.classList.add("hidden");
      if (d.type === "dir" && d.repoId && d.path) {
        fetch("/api/v2.1/upload-links/?repo_id=" + encodeURIComponent(d.repoId) + "&path=" + encodeURIComponent(d.path))
          .then(function (r) { return r.json(); })
          .then(function (data) {
            var links = data.upload_link_list || [];
            ulList.innerHTML = "";
            if (links.length === 0) {
              noUl.classList.remove("hidden");
            } else {
              links.forEach(function (link) {
                var div = document.createElement("div");
                div.className = "flex items-center justify-between py-0.5";
                var linkUrl = link.link || "/u/" + link.token + "/";
                div.innerHTML =
                  '<a href="' + escapeHtml(linkUrl) + '" target="_blank" class="text-xs text-emerald-500 hover:text-emerald-600 truncate block">' +
                    escapeHtml(link.token || "") +
                  '</a>' +
                  '<span class="text-xs text-gray-400 flex-shrink-0 ml-2">' + (link.view_cnt || 0) + ' uploads</span>';
                ulList.appendChild(div);
              });
            }
            ulSection.classList.remove("hidden");
          })
          .catch(function () { /* ignore */ });
      }
    }

    // ── Indexed Content ──
    var indexSection = ct.querySelector(".js-rp-index-section");
    var indexContent = ct.querySelector(".js-rp-index-content");
    var indexEmpty = ct.querySelector(".js-rp-index-empty");
    var reindexBtn = ct.querySelector(".js-rp-reindex-btn");

    if (indexSection && d.type !== "dir" && d.repoId && d.path) {
      indexSection.classList.remove("hidden");
      if (reindexBtn) {
        reindexBtn.dataset.repoId = d.repoId;
        reindexBtn.dataset.path = d.path;
        reindexBtn.disabled = false;
        reindexBtn.textContent = "Reindex";
      }
      fetch("/api2/repos/" + encodeURIComponent(d.repoId) + "/file/index-text/?p=" + encodeURIComponent(d.path))
        .then(function (r) { return r.json(); })
        .then(function (data) {
          if (data.content) {
            indexContent.textContent = data.content;
            indexContent.classList.remove("hidden");
            if (indexEmpty) indexEmpty.classList.add("hidden");
          } else {
            indexContent.classList.add("hidden");
            if (indexEmpty) indexEmpty.classList.remove("hidden");
          }
        })
        .catch(function () { /* ignore */ });
    } else if (indexSection) {
      indexSection.classList.add("hidden");
    }

    // ── EXIF Data (image files only) ──
    var exifSection = ct.querySelector(".js-rp-exif-section");
    var exifContent = ct.querySelector(".js-rp-exif-content");
    var noExif = ct.querySelector(".js-rp-no-exif");

    if (exifSection && d.type !== "dir" && d.thumbnailUrl && d.repoId && d.path) {
      fetch("/api2/repos/" + encodeURIComponent(d.repoId) + "/file/exif/?p=" + encodeURIComponent(d.path))
        .then(function (r) { return r.json(); })
        .then(function (data) {
          exifContent.innerHTML = "";
          if (data && typeof data === "object" && !Array.isArray(data)) {
            var fields = getExifFields(data);
            var hasData = false;
            fields.forEach(function (f) {
              hasData = true;
              var div = document.createElement("div");
              div.className = "flex items-center justify-between";
              div.innerHTML = '<span class="text-xs text-gray-500 dark:text-gray-400">' + f.label + '</span>' +
                '<span class="text-xs font-medium text-gray-900 dark:text-gray-100 text-right">' + escapeHtml(f.value) + '</span>';
              exifContent.appendChild(div);
            });
            if (hasData) {
              exifSection.classList.remove("hidden");
              if (noExif) noExif.classList.add("hidden");
            } else {
              exifSection.classList.add("hidden");
            }
          } else {
            exifSection.classList.add("hidden");
            if (noExif) noExif.classList.remove("hidden");
          }
        })
        .catch(function () { /* ignore */ });
    } else if (exifSection) {
      exifSection.classList.add("hidden");
    }
  };

  // ─── Multi-select right panel ──────────────────────────────────────
  window.openMultiSelectPanel = function (selectedItems) {
    // selectedItems = [{ name, type }, ...]
    var ph = document.querySelector(".js-rp-placeholder");
    var ct = document.querySelector(".js-rp-content");
    var mc = document.querySelector(".js-rp-multi-content");
    if (!ph || !ct || !mc) return;

    ph.classList.add("hidden");
    ct.classList.add("hidden");
    mc.classList.remove("hidden");

    var countEl = mc.querySelector(".js-rp-multi-count");
    if (countEl) countEl.textContent = selectedItems.length + " item(s) selected";

    var listEl = mc.querySelector(".js-rp-multi-list");
    if (listEl) {
      listEl.innerHTML = "";
      selectedItems.forEach(function (item) {
        var div = document.createElement("div");
        div.className = "flex items-center gap-2 py-0.5";
        // Folder icon or file extension badge
        if (item.type === "dir") {
          var iconSpan = document.createElement("span");
          iconSpan.className = "h-5 w-5 flex-shrink-0 flex items-center justify-center";
          iconSpan.innerHTML = '<svg class="h-4 w-4 text-amber-400" fill="currentColor" viewBox="0 0 24 24"><path d="M2 6a2 2 0 012-2h5l2 2h9a2 2 0 012 2v10a2 2 0 01-2 2H4a2 2 0 01-2-2V6z"/></svg>';
          div.appendChild(iconSpan);
        } else {
          var badgeSpan = document.createElement("span");
          badgeSpan.className = "h-5 w-5 flex-shrink-0 rounded bg-gray-100 dark:bg-surface-700 flex items-center justify-center text-[9px] leading-none font-semibold text-gray-500 dark:text-gray-400";
          badgeSpan.textContent = "F";
          div.appendChild(badgeSpan);
        }
        var nameSpan = document.createElement("span");
        nameSpan.className = "text-xs text-gray-900 dark:text-gray-100 truncate";
        nameSpan.textContent = item.name + (item.type === "dir" ? "/" : "");
        div.appendChild(nameSpan);
        listEl.appendChild(div);
      });
    }
  };

  // Reset right panel to placeholder state
  window.resetRightPanel = function () {
    var ph = document.querySelector(".js-rp-placeholder");
    var ct = document.querySelector(".js-rp-content");
    var mc = document.querySelector(".js-rp-multi-content");
    if (ph) ph.classList.remove("hidden");
    if (ct) ct.classList.add("hidden");
    if (mc) mc.classList.add("hidden");
  };

  // ─── Helpers ─────────────────────────────────────────────────────────
  function setText(container, selector, val) {
    var el = container.querySelector(selector);
    if (el) el.textContent = val;
  }

  // Map EXIF field names to human-readable labels and format values.
  function getExifFields(data) {
    var labelMap = {
      "Make": "Camera Make",
      "Model": "Camera Model",
      "DateTimeOriginal": "Date Taken",
      "ExposureTime": "Exposure",
      "FNumber": "Aperture",
      "FocalLength": "Focal Length",
      "ISOSpeed": "ISO",
      "Flash": "Flash",
      "Software": "Software",
      "GPSLatitude": "GPS Latitude",
      "GPSLongitude": "GPS Longitude",
      "PixelXDimension": "Width",
      "PixelYDimension": "Height",
      "Orientation": "Orientation"
    };
    // Simple value formatters for certain fields
    var formatters = {
      "ISOSpeed": function (v) { return v.replace(/^"|"$/g, ""); },
      "ExposureTime": function (v) { return v.replace(/^"|"$/g, ""); },
      "FNumber": function (v) { return v.replace(/^"|"$/g, "").replace(/^F\//, "f/"); },
      "FocalLength": function (v) { return v.replace(/^"|"$/g, ""); },
      "Flash": function (v) {
        var val = parseInt(v, 10);
        if (isNaN(val)) return v;
        // Bit 0: flash fired
        return (val & 1) ? "Yes" : "No";
      },
      "PixelXDimension": function (v) { return v.replace(/^"|"$/g, "") + " px"; },
      "PixelYDimension": function (v) { return v.replace(/^"|"$/g, "") + " px"; },
      "DateTimeOriginal": function (v) { return v.replace(/^"|"$/g, ""); },
      "Make": function (v) { return v.replace(/^"|"$/g, ""); },
      "Model": function (v) { return v.replace(/^"|"$/g, ""); },
      "Software": function (v) { return v.replace(/^"|"$/g, ""); },
      "GPSLatitude": function (v) { return v.replace(/^"|"$/g, ""); },
      "GPSLongitude": function (v) { return v.replace(/^"|"$/g, ""); },
      "Orientation": function (v) {
        var m = {
          "1": "Normal",
          "2": "Mirrored",
          "3": "Upside-down",
          "4": "Rotated 180°",
          "5": "Mirrored + 90° CW",
          "6": "90° CW",
          "7": "Mirrored + 90° CCW",
          "8": "90° CCW"
        };
        var val = v.replace(/^"|"$/g, "");
        return m[val] || v;
      }
    };
    var order = [
      "Make", "Model", "DateTimeOriginal",
      "ExposureTime", "FNumber", "ISOSpeed", "FocalLength", "Flash",
      "Software",
      "GPSLatitude", "GPSLongitude",
      "PixelXDimension", "PixelYDimension",
      "Orientation"
    ];
    var result = [];
    for (var i = 0; i < order.length; i++) {
      var key = order[i];
      var raw = data[key];
      if (raw === undefined || raw === null) continue;
      var label = labelMap[key] || key;
      var value = formatters[key] ? formatters[key](raw) : raw;
      result.push({ label: label, value: value });
    }
    return result;
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

  // ─── Reindex single file ───────────────────────────────────────────────
  document.addEventListener("click", async function (e) {
    var btn = e.target.closest(".js-rp-reindex-btn");
    if (!btn) return;
    var repoId = btn.dataset.repoId;
    var path = btn.dataset.path;
    if (!repoId || !path) return;
    try {
      btn.disabled = true;
      btn.textContent = "Indexing...";
      var resp = await window.apiFetch("/api2/repos/" + encodeURIComponent(repoId) + "/file/reindex/", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ p: path }),
      });
      var result = await resp.json();
      if (result.indexed) {
        window.Toast && Toast.success("Reindexed");
      } else {
        window.Toast && Toast.info("File type not supported for indexing");
      }
      // Reload the indexed content display
      var ct = document.querySelector(".js-rp-content");
      if (ct) {
        var indexContent = ct.querySelector(".js-rp-index-content");
        var indexEmpty = ct.querySelector(".js-rp-index-empty");
        var fetchResp = await fetch("/api2/repos/" + encodeURIComponent(repoId) + "/file/index-text/?p=" + encodeURIComponent(path));
        var fetchData = await fetchResp.json();
        if (fetchData.content) {
          indexContent.textContent = fetchData.content;
          indexContent.classList.remove("hidden");
          if (indexEmpty) indexEmpty.classList.add("hidden");
        } else {
          indexContent.classList.add("hidden");
          if (indexEmpty) indexEmpty.classList.remove("hidden");
        }
      }
    } catch (e) {
      window.Toast && Toast.error("Reindex failed: " + (e.message || e));
    } finally {
      btn.textContent = "Reindex";
      btn.disabled = false;
    }
  });

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
