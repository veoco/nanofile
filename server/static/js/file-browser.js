// Nanofile Web UI — File browser specific JS
(function () {
  "use strict";

  // ─── Translation helper ────────────────────────────────────────────────
  // `__t` and `window.__t` are defined in main.js (loaded first in base.html);
  // this file reuses that global implementation.
  var __t = window.__t;

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

  // ─── Tag filter ──────────────────────────────────────────────────────
  window.getTagFilter = function () {
    var sb = document.querySelector(".js-sort-bar");
    return sb ? (sb.dataset.tagFilter || "") : "";
  };

  function applyTagFilter(name) {
    var sb = document.querySelector(".js-sort-bar");
    if (!sb) return;
    var current = sb.dataset.tagFilter || "";
    sb.dataset.tagFilter = current === name ? "" : name;
    if (typeof window.refreshFileList === "function") window.refreshFileList();
  }

  document.addEventListener("click", function (e) {
    var btn = e.target.closest(".js-tag-filter-btn");
    if (btn) { e.stopPropagation(); applyTagFilter(btn.dataset.tag); return; }
    var entryTag = e.target.closest(".js-entry-tag");
    if (entryTag) { e.stopPropagation(); applyTagFilter(entryTag.dataset.tag); }
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
    var videoEl = ct.querySelector(".js-rp-video");
    var audioIcon = ct.querySelector(".js-rp-audio-icon");
    var audioRow = ct.querySelector(".js-rp-audio-row");
    var audioEl = ct.querySelector(".js-rp-audio");

    // Stop any previously-playing media before switching selection.
    stopRightPanelMedia();

    // Hide all preview variants first
    if (thumbImg) { thumbImg.classList.add("hidden"); thumbImg.style.display = ""; thumbImg.removeAttribute("src"); }
    if (extBadge) extBadge.classList.add("hidden");
    if (folderIcon) folderIcon.classList.add("hidden");
    if (videoIcon) videoIcon.classList.add("hidden");
    if (videoEl) videoEl.classList.add("hidden");
    if (audioIcon) audioIcon.classList.add("hidden");
    if (audioRow) audioRow.classList.add("hidden");

    if (d.type === "dir") {
      if (folderIcon) folderIcon.classList.remove("hidden");
    } else if (d.isVideo) {
      // Inline playback via the Range-capable streaming endpoint; the frame
      // thumbnail (if any) doubles as the native poster.
      if (videoEl && d.repoId && d.path) {
        var encPath = d.path.split("/").map(encodeURIComponent).join("/");
        videoEl.src = "/libraries/" + encodeURIComponent(d.repoId) + "/files/" + encPath;
        videoEl.poster = d.thumbnailUrlLarge || d.thumbnailUrl || "";
        videoEl.classList.remove("hidden");
      }
    } else if (d.isAudio) {
      // Cover art (if any) as the poster; otherwise a music note. The player
      // bar sits just below the preview box.
      if (audioRow && d.repoId && d.path) {
        audioEl.src = "/libraries/" + encodeURIComponent(d.repoId) + "/files/" +
          d.path.split("/").map(encodeURIComponent).join("/");
        audioRow.classList.remove("hidden");
      }
      if (d.thumbnailUrlLarge || d.thumbnailUrl) {
        if (thumbImg) { thumbImg.dataset.extension = d.extension || ""; thumbImg.src = d.thumbnailUrlLarge || d.thumbnailUrl; thumbImg.classList.remove("hidden"); }
      } else if (audioIcon) {
        audioIcon.classList.remove("hidden");
      }
    } else if (d.thumbnailUrlLarge || d.thumbnailUrl) {
      if (thumbImg) { thumbImg.dataset.extension = d.extension || ""; thumbImg.src = d.thumbnailUrlLarge || d.thumbnailUrl; thumbImg.classList.remove("hidden"); }
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
      if (starLabel) starLabel.textContent = isStarred ? __t('ui.starred') : __t('ui.not_starred');
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

    // History (only meaningful for files)
    var historyBtn = ct.querySelector(".js-rp-history-btn");
    if (historyBtn) {
      if (d.type === "file") {
        historyBtn.dataset.repoId = d.repoId || "";
        historyBtn.dataset.path = d.path || "";
        historyBtn.classList.remove("hidden");
      } else {
        historyBtn.classList.add("hidden");
      }
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
            var links = data || [];
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

    // ── Tags (fetch for the selected item) ──
    var tagsSection = ct.querySelector(".js-rp-tags-section");
    var tagsList = ct.querySelector(".js-rp-tags-list");
    var tagInput = ct.querySelector(".js-rp-tag-input");
    var tagDatalist = ct.querySelector("#js-rp-tag-options");
    var noTags = ct.querySelector(".js-rp-no-tags");
    var addTagBtn = ct.querySelector(".js-rp-tag-add");
    if (tagsSection && tagsList && d.repoId && d.recordId) {
      tagsSection.classList.add("hidden");
      tagsList.innerHTML = "";
      noTags.classList.add("hidden");

      var repoId = d.repoId;
      var recordId = d.recordId;
      var allTags = [];   // [{id, name, color}]
      var fileTagIds = []; // tag ids currently attached

      function renderTagChips() {
        tagsList.innerHTML = "";
        noTags.classList.toggle("hidden", fileTagIds.length > 0);
        fileTagIds.forEach(function (tid) {
          var tag = allTags.find(function (t) { return String(t.id) === String(tid); });
          if (!tag) return;
          var chip = document.createElement("span");
          chip.className = "js-rp-tag-chip inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-gray-700 dark:text-gray-200";
          chip.style.backgroundColor = (tag.color || "#e6e6e6") + "33";
          chip.innerHTML =
            '<span class="inline-block h-1.5 w-1.5 rounded-full" style="background-color:' + escapeHtml(tag.color || "#e6e6e6") + ';"></span>' +
            escapeHtml(tag.name) +
            '<button type="button" class="js-rp-tag-remove hover:text-red-500" data-tag-id="' + encodeURIComponent(tag.id) + '" title="' + escapeHtml(__t('fb.remove_tag')) + '">' +
            '  <svg class="h-2.5 w-2.5" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12"/></svg>' +
            "</button>";
          tagsList.appendChild(chip);
        });
      }

      function saveTags(nextTagIds) {
        return window.apiFetch(
          "/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/file-tags/",
          {
            method: "PUT",
            body: JSON.stringify({ file_tags_data: [{ record_id: recordId, tags: nextTagIds }] }),
          }
        ).then(function () {
          fileTagIds = nextTagIds;
          renderTagChips();
          if (typeof window.refreshFileList === "function") window.refreshFileList();
        });
      }

      // Load repo tags + this file's current tags.
      var pathForQuery = d.path || "/" + (d.name || "");
      var slash = pathForQuery.lastIndexOf("/");
      var parentDir = slash <= 0 ? "/" : pathForQuery.slice(0, slash);
      var fileName = slash <= 0 ? pathForQuery.replace(/^\//, "") : pathForQuery.slice(slash + 1);

      Promise.all([
        window.apiFetch("/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/tags/?start=0&limit=1000").then(function (r) { return r.json(); }),
        window.apiFetch("/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/record/?parent_dir=" + encodeURIComponent(parentDir) + "&name=" + encodeURIComponent(fileName) + "&file_name=" + encodeURIComponent(fileName)).then(function (r) { return r.json(); }),
      ]).then(function (results) {
        var tagData = results[0] || {};
        var recData = results[1] || {};
        allTags = (tagData.results || []).map(function (t) {
          return { id: t._id, name: t._tag_name, color: t._tag_color };
        });
        if (tagDatalist) {
          tagDatalist.innerHTML = "";
          allTags.forEach(function (t) {
            var opt = document.createElement("option");
            opt.value = t.name;
            tagDatalist.appendChild(opt);
          });
        }
        var rec = (recData.results || [])[0] || {};
        fileTagIds = (rec._tags || []).map(function (l) { return l.row_id; });
        renderTagChips();
        tagsSection.classList.remove("hidden");
      }).catch(function () {
        tagsSection.classList.add("hidden");
      });

      if (addTagBtn && tagInput) {
        addTagBtn.onclick = function () {
          var name = (tagInput.value || "").trim();
          if (!name) return;
          var existing = allTags.find(function (t) { return t.name === name; });
          var p;
          if (existing) {
            p = Promise.resolve(existing.id);
          } else {
            var colors = ["#ff9800", "#f44336", "#4caf50", "#2196f3", "#9c27b0", "#00bcd4", "#ffeb3b", "#8bc34a", "#ff5722", "#3f51b5"];
            var color = colors[Math.floor(Math.random() * colors.length)];
            p = window.apiFetch("/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/tags/", {
              method: "POST",
              body: JSON.stringify({ tags_data: [{ _tag_name: name, _tag_color: color }] }),
            }).then(function (r) { return r.json(); }).then(function (data) {
              var created = (data.tags || [])[0];
              allTags.push({ id: created._id, name: created._tag_name, color: created._tag_color });
              if (tagDatalist) {
                var opt = document.createElement("option");
                opt.value = created._tag_name;
                tagDatalist.appendChild(opt);
              }
              return created._id;
            });
          }
          p.then(function (tid) {
            var next = fileTagIds.slice();
            if (next.indexOf(tid) === -1) next.push(tid);
            tagInput.value = "";
            return saveTags(next);
          }).catch(function () {
            if (window.Toast && window.Toast.error) window.Toast.error(__t('fb.add_tag_failed'));
          });
        };

        tagsList.onclick = function (e) {
          var rm = e.target.closest(".js-rp-tag-remove");
          if (!rm) return;
          var tid = decodeURIComponent(rm.dataset.tagId);
          var next = fileTagIds.filter(function (id) { return String(id) !== String(tid); });
          saveTags(next).catch(function () {
            if (window.Toast && window.Toast.error) window.Toast.error(__t('fb.remove_tag_failed'));
          });
        };
      }
    } else if (tagsSection) {
      tagsSection.classList.add("hidden");
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
            var links = data || [];
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
        reindexBtn.textContent = __t('ui.reindex');
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

    if (exifSection && d.type !== "dir" && !d.isVideo && !d.isAudio && d.thumbnailUrl && d.repoId && d.path) {
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

  // Pause and unload any media element currently playing in the right panel.
  function stopRightPanelMedia() {
    var v = document.querySelector(".js-rp-video");
    if (v) {
      v.pause();
      v.removeAttribute("src");
      v.load();
      v.poster = "";
    }
    var a = document.querySelector(".js-rp-audio");
    if (a) {
      a.pause();
      a.removeAttribute("src");
      a.load();
    }
  }

  // Called when a thumbnail <img> fails to load (e.g. audio without cover art,
  // or ffmpeg unavailable) — fall back to the extension badge next to it.
  window.thumbFailed = function (img) {
    img.style.display = "none";
    // Ignore errors for thumbnails that are not part of the active preview (e.g.
    // a just-cleared right-panel thumbnail); otherwise the fallback badge below
    // would be revealed for the previously selected file.
    if (img.classList.contains("hidden")) return;
    // List thumbnails: show the small extension fallback badge next to the icon.
    var fb = img.parentElement ? img.parentElement.querySelector(".js-thb-fallback") : null;
    if (fb) {
      fb.classList.remove("hidden");
      fb.classList.add("flex");
      return;
    }
    // Right-panel thumbnails (e.g. audio without cover art): fall back to the
    // large extension badge, same as unknown files.
    var extBadge = document.querySelector(".js-rp-content .js-rp-ext-badge");
    if (extBadge) {
      extBadge.textContent = img.dataset && img.dataset.extension ? img.dataset.extension : "?";
      extBadge.classList.remove("hidden");
    }
  };

  // Reset right panel to placeholder state
  window.resetRightPanel = function () {
    var ph = document.querySelector(".js-rp-placeholder");
    var ct = document.querySelector(".js-rp-content");
    var mc = document.querySelector(".js-rp-multi-content");
    // Stop any playing media so it doesn't keep buffering in the background.
    stopRightPanelMedia();
    if (ph) ph.classList.remove("hidden");
    if (ct) ct.classList.add("hidden");
    if (mc) mc.classList.add("hidden");
  };

  // ─── Quick preview modal (dblclick on a file row) ─────────────────────
  var QUICK_PREVIEW_IMAGE_EXTS = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tiff", "tif", "heic", "heif", "avif"];
  var QUICK_PREVIEW_TEXT_LIMIT = 1024 * 1024; // 1MB

  function isQuickPreviewImage(name) {
    var i = name.lastIndexOf(".");
    if (i === -1) return false;
    return QUICK_PREVIEW_IMAGE_EXTS.indexOf(name.slice(i + 1).toLowerCase()) !== -1;
  }

  function showQuickPreviewUnsupported() {
    var overlay = document.getElementById("quick-preview-overlay");
    if (!overlay) return;
    var unsupported = overlay.querySelector(".js-qp-unsupported");
    if (unsupported) {
      unsupported.textContent = __t("fb.preview_failed");
      unsupported.classList.remove("hidden");
    }
  }

  function resetQuickPreview() {
    var overlay = document.getElementById("quick-preview-overlay");
    if (!overlay) return;
    var img = overlay.querySelector(".js-qp-img");
    var video = overlay.querySelector(".js-qp-video");
    var audio = overlay.querySelector(".js-qp-audio");
    var text = overlay.querySelector(".js-qp-text");
    var unsupported = overlay.querySelector(".js-qp-unsupported");
    if (video) { video.pause(); video.removeAttribute("src"); video.load(); }
    if (audio) { audio.pause(); audio.removeAttribute("src"); audio.load(); }
    if (img) { img.removeAttribute("src"); img.onerror = null; }
    if (text) text.textContent = "";
    [img, video, audio, text, unsupported].forEach(function (el) {
      if (el) el.classList.add("hidden");
    });
  }

  window.hideQuickPreview = function () {
    var overlay = document.getElementById("quick-preview-overlay");
    if (!overlay) return;
    resetQuickPreview();
    overlay.classList.add("hidden");
  };

  window.openQuickPreview = function (row) {
    var overlay = document.getElementById("quick-preview-overlay");
    if (!overlay) return;
    resetQuickPreview();

    var repoId = row.dataset.repoId;
    var path = row.dataset.path;
    var name = row.dataset.name || "";
    var isVideo = row.dataset.isVideo === "true";
    var isAudio = row.dataset.isAudio === "true";
    var isPreviewable = row.dataset.isPreviewable === "true";
    if (!repoId || !path) return;

    var encPath = path.split("/").map(encodeURIComponent).join("/");
    var title = overlay.querySelector(".js-qp-title");
    if (title) title.textContent = name;

    var img = overlay.querySelector(".js-qp-img");
    var video = overlay.querySelector(".js-qp-video");
    var audio = overlay.querySelector(".js-qp-audio");
    var text = overlay.querySelector(".js-qp-text");

    if (isVideo) {
      video.src = "/libraries/" + encodeURIComponent(repoId) + "/files/" + encPath;
      video.classList.remove("hidden");
    } else if (isAudio) {
      audio.src = "/libraries/" + encodeURIComponent(repoId) + "/files/" + encPath;
      audio.classList.remove("hidden");
    } else if (isPreviewable && isQuickPreviewImage(name)) {
      img.src = "/repos/" + encodeURIComponent(repoId) + "/files/" + encPath;
      img.onerror = function () {
        img.classList.add("hidden");
        showQuickPreviewUnsupported();
      };
      img.classList.remove("hidden");
    } else if (isPreviewable) {
      // Text / code — fetch the first 1MB via Range; huge files are truncated.
      var fileSize = parseInt(row.dataset.size, 10);
      var url = "/repos/" + encodeURIComponent(repoId) + "/files/" + encPath;
      fetch(url, { headers: { Range: "bytes=0-" + (QUICK_PREVIEW_TEXT_LIMIT - 1) } })
        .then(function (res) {
          if (!res.ok) { showQuickPreviewUnsupported(); return null; }
          return res.text();
        })
        .then(function (content) {
          if (content === null) return;
          text.textContent = content;
          if (!isNaN(fileSize) && fileSize > QUICK_PREVIEW_TEXT_LIMIT) {
            text.textContent += "\n\n-- (truncated, showing first 1MB) --";
          }
          text.classList.remove("hidden");
        })
        .catch(function () { showQuickPreviewUnsupported(); });
    } else {
      showQuickPreviewUnsupported();
    }

    overlay.classList.remove("hidden");
  };

  // Quick preview modal event bindings (close button, backdrop, ESC).
  (function () {
    var overlay = document.getElementById("quick-preview-overlay");
    if (!overlay) return;
    var close = overlay.querySelector(".js-qp-close");
    if (close) close.addEventListener("click", function () { window.hideQuickPreview(); });
    overlay.addEventListener("click", function (e) {
      if (e.target === overlay) window.hideQuickPreview();
    });
  })();

  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") {
      var overlay = document.getElementById("quick-preview-overlay");
      if (overlay && !overlay.classList.contains("hidden")) window.hideQuickPreview();
    }
  });

  // ─── Helpers ─────────────────────────────────────────────────────────
  function setText(container, selector, val) {
    var el = container.querySelector(selector);
    if (el) el.textContent = val;
  }

  // Map EXIF field names to human-readable labels and format values.
  function getExifFields(data) {
    var labelMap = {
      "Make": __t('exif.make'),
      "Model": __t('exif.model'),
      "DateTimeOriginal": __t('exif.date_taken'),
      "ExposureTime": __t('exif.exposure'),
      "FNumber": __t('exif.aperture'),
      "FocalLength": __t('exif.focal_length'),
      "ISOSpeed": __t('exif.iso'),
      "Flash": __t('exif.flash'),
      "Software": __t('exif.software'),
      "GPSLatitude": __t('exif.gps_latitude'),
      "GPSLongitude": __t('exif.gps_longitude'),
      "PixelXDimension": __t('exif.width'),
      "PixelYDimension": __t('exif.height'),
      "Orientation": __t('exif.orientation')
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
        return (val & 1) ? __t('common.yes') : __t('common.no');
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
          "1": __t('exif.orientation_normal'),
          "2": __t('exif.orientation_mirrored'),
          "3": __t('exif.orientation_upside_down'),
          "4": __t('exif.orientation_rotated_180'),
          "5": __t('exif.orientation_mirrored_90_cw'),
          "6": __t('exif.orientation_90_cw'),
          "7": __t('exif.orientation_mirrored_90_ccw'),
          "8": __t('exif.orientation_90_ccw')
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
    if (type === "dir") return __t('ft.folder');
    if (!ext) return __t('ft.file');
    var map = {
      "PNG": __t('ft.png_image'), "JPG": __t('ft.jpeg_image'), "JPEG": __t('ft.jpeg_image'),
      "GIF": __t('ft.gif_image'), "WEBP": __t('ft.webp_image'), "BMP": __t('ft.bmp_image'),
      "SVG": __t('ft.svg_image'),
      "PDF": __t('ft.pdf_document'),
      "DOC": __t('ft.word_document'), "DOCX": __t('ft.word_document'),
      "XLS": __t('ft.excel_spreadsheet'), "XLSX": __t('ft.excel_spreadsheet'),
      "PPT": __t('ft.powerpoint'), "PPTX": __t('ft.powerpoint'),
      "TXT": __t('ft.text_file'), "MD": __t('ft.markdown_file'),
      "RS": __t('ft.rust_source'), "PY": __t('ft.python_script'), "JS": __t('ft.javascript_file'),
      "TS": __t('ft.typescript_file'), "GO": __t('ft.go_source'), "JAVA": __t('ft.java_source'),
      "C": __t('ft.c_source'), "CPP": __t('ft.cpp_source'), "H": __t('ft.header_file'),
      "RB": __t('ft.ruby_script'), "PHP": __t('ft.php_script'), "SH": __t('ft.shell_script'),
      "HTML": __t('ft.html_file'), "CSS": __t('ft.css_file'),
      "TOML": __t('ft.toml_file'), "JSON": __t('ft.json_file'), "YAML": __t('ft.yaml_file'), "YML": __t('ft.yaml_file'),
      "CSV": __t('ft.csv_file'), "XML": __t('ft.xml_file'), "SQL": __t('ft.sql_file'),
      "ZIP": __t('ft.zip_archive'), "TAR": __t('ft.tar_archive'), "GZ": __t('ft.gz_archive'),
      "BZ2": __t('ft.bz2_archive'), "7Z": __t('ft.sevenzip_archive'), "RAR": __t('ft.rar_archive'),
      "MP4": __t('ft.mp4_video'), "MOV": __t('ft.mov_video'), "AVI": __t('ft.avi_video'),
      "MKV": __t('ft.mkv_video'), "WEBM": __t('ft.webm_video'), "WMV": __t('ft.wmv_video'),
      "FLV": __t('ft.flv_video'), "3GP": __t('ft.3gp_video'),
      "MP3": __t('ft.mp3_audio'), "FLAC": __t('ft.flac_audio'), "WAV": __t('ft.wav_audio'),
      "OGG": __t('ft.ogg_audio'), "M4A": __t('ft.m4a_audio'), "AAC": __t('ft.aac_audio'),
      "WMA": __t('ft.wma_audio'), "OPUS": __t('ft.opus_audio'),
      "ISO": __t('ft.disk_image')
    };
    return map[ext] || ext + " File";
  }

  // ─── Repo filter ────────────────────────────────────────────────────
  var repoFilter = document.querySelector(".js-repo-filter");
  if (repoFilter) {
    // Debounce so a fast typist isn't re-filtering a large repo list on
    // every keystroke.
    var filterTimer = null;
    repoFilter.addEventListener("input", function () {
      clearTimeout(filterTimer);
      filterTimer = setTimeout(function () {
        var q = repoFilter.value.toLowerCase();
        var items = document.querySelectorAll(".js-repo-item");
        for (var i = 0; i < items.length; i++) {
          var name = (items[i].textContent || "").toLowerCase();
          items[i].style.display = name.indexOf(q) > -1 ? "" : "none";
        }
      }, 60);
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
        else { r.json().then(function (e) { window.Toast && Toast.error(e.error_msg || __t('ui.failed')); }); }
      })
      .catch(function () { window.Toast && Toast.error(__t('ui.network_error')); });
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
      btn.textContent = __t('ui.reindex');
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
