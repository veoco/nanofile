// right-panel — the file-detail side panel (preview, details, tags, share &
// upload links, index text, EXIF), multi-select panel, and quick preview modal.
import { __t } from "../core/i18n.js";
import { escapeHtml, escapeAttr, encodeFilePath, safeColor, parentDirOf } from "../core/utils.js";
import { apiFetch } from "../core/api.js";
import { humanType, isQuickPreviewImage, getExifFields } from "../core/file-meta.js";
import { Toast } from "../core/toast.js";
import { refreshFileList } from "./list.js";

// Monotonic id used to discard stale right-panel async responses when the
// selection changes faster than the detail requests (share/upload links,
// index text, EXIF, tags) resolve.
var rpReqId = 0;

export function openRightPanel(d) {
  // d = { name, type, starred, extension, path, repoId, modifierEmail,
  //       thumbnailUrl, thumbnailUrlLarge, isPreviewable, downloadUrl, isVideo }

  var ph = document.querySelector(".js-rp-placeholder");
  var ct = document.querySelector(".js-rp-content");
  var mc = document.querySelector(".js-rp-multi-content");
  if (!ph || !ct) return;

  // Invalidate any in-flight async detail requests from a previous selection.
  var reqId = ++rpReqId;

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
      var encPath = encodeFilePath(d.path);
      videoEl.src = "/repos/" + encodeURIComponent(d.repoId) + "/files/" + encPath;
      videoEl.poster = d.thumbnailUrlLarge || d.thumbnailUrl || "";
      videoEl.classList.remove("hidden");
    }
  } else if (d.isAudio) {
    // Cover art (if any) as the poster; otherwise a music note. The player
    // bar sits just below the preview box.
    if (audioRow && d.repoId && d.path) {
      audioEl.src = "/repos/" + encodeURIComponent(d.repoId) + "/files/" +
        encodeFilePath(d.path);
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

  // ── Size ──
  setText(ct, ".js-rp-size", d.type === "dir" ? "-" : (d.sizeDisplay || ""));

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
  if (shareSection && shareList) {
    if (d.repoId && d.path) {
      // Show the section right away with the loading hint in the reserved
      // (min-h-5) list row, so the panel layout doesn't shift when the links
      // arrive; only the list content is swapped, not the whole section.
      shareSection.classList.remove("hidden");
      shareList.innerHTML = '<div class="js-rp-share-links-loading text-xs text-gray-400 dark:text-gray-500 italic">' + escapeHtml(__t('fb.loading')) + '</div>';
      fetch("/api/v2.1/share-links/?repo_id=" + encodeURIComponent(d.repoId) + "&path=" + encodeURIComponent(d.path))
        .then(function (r) { return r.json(); })
        .then(function (data) {
          if (reqId !== rpReqId) return; // stale response
          var links = data || [];
          shareList.innerHTML = "";
          if (links.length === 0) {
            shareList.innerHTML = '<div class="js-rp-no-share-links text-xs text-gray-400 dark:text-gray-500 italic">' + escapeHtml(__t('fb.no_share_links')) + '</div>';
          } else {
            links.forEach(function (link) {
              var div = document.createElement("div");
              div.className = "flex items-center justify-between py-0.5";
              div.innerHTML =
                '<a href="' + escapeAttr(link.link || "") + '" target="_blank" class="text-xs text-brand-500 hover:text-brand-600 truncate block">' +
                  escapeHtml(link.token || "") +
                '</a>' +
                '<span class="text-xs text-gray-400 flex-shrink-0 ml-2">' + (link.view_cnt || 0) + ' views</span>';
              shareList.appendChild(div);
            });
          }
        })
        .catch(function () { /* ignore */ });
    } else {
      // No repo/path context for this item — keep the section hidden.
      shareSection.classList.add("hidden");
    }
  }

  // ── Tags (fetch for the selected item) ──
  var tagsSection = ct.querySelector(".js-rp-tags-section");
  var tagsList = ct.querySelector(".js-rp-tags-list");
  var tagInput = ct.querySelector(".js-rp-tag-input");
  var tagDatalist = ct.querySelector("#js-rp-tag-options");
  var addTagBtn = ct.querySelector(".js-rp-tag-add");
  if (tagsSection && tagsList && d.repoId && d.recordId) {
    // Keep the section visible while tags load so the panel layout doesn't
    // shift when they arrive; the chip row height is reserved via min-h.
    tagsSection.classList.remove("hidden");
    tagsList.innerHTML = "";
    if (tagInput) tagInput.value = "";
    var tagsReqId = reqId; // discard stale responses from earlier selections

    var repoId = d.repoId;
    var recordId = d.recordId;
    var allTags = [];   // [{id, name, color}]
    var fileTagIds = []; // tag ids currently attached
    // Set once the user saves a change; the initial load response (which
    // reflects pre-save state) must not roll the panel back over it.
    var userMutated = false;

    function renderTagChips() {
      tagsList.innerHTML = "";
      // When the file has no tags, render the hint inside the chip row (the
      // slot that would hold chips) so there's no blank reserved area and the
      // row height stays stable — no separate "no tags" line below the input.
      if (fileTagIds.length === 0) {
        tagsList.innerHTML =
          '<span class="js-rp-no-tags text-xs text-gray-400 dark:text-gray-500 italic">' +
          escapeHtml(__t('fb.no_tags')) +
          "</span>";
        return;
      }
      fileTagIds.forEach(function (tid) {
        var tag = allTags.find(function (t) { return String(t.id) === String(tid); });
        if (!tag) return;
        var chip = document.createElement("span");
        chip.className = "js-rp-tag-chip inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-gray-700 dark:text-gray-200";
        var tagColor = safeColor(tag.color);
        chip.style.backgroundColor = tagColor + "33";
        chip.innerHTML =
          '<span class="inline-block h-1.5 w-1.5 rounded-full" style="background-color:' + escapeAttr(tagColor) + ';"></span>' +
          escapeHtml(tag.name) +
          '<button type="button" class="js-rp-tag-remove hover:text-red-500" data-tag-id="' + encodeURIComponent(tag.id) + '" title="' + escapeAttr(__t('fb.remove_tag')) + '">' +
          '  <svg class="h-2.5 w-2.5" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12"/></svg>' +
          "</button>";
        tagsList.appendChild(chip);
      });
    }

    function saveTags(nextTagIds) {
      return apiFetch(
        "/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/file-tags/",
        {
          method: "PUT",
          body: JSON.stringify({ file_tags_data: [{ record_id: recordId, tags: nextTagIds }] }),
        }
      ).then(function () {
        userMutated = true;
        fileTagIds = nextTagIds;
        renderTagChips();
        refreshFileList();
      });
    }

    // Load repo tags + this file's current tags.
    var pathForQuery = d.path || "/" + (d.name || "");
    var slash = pathForQuery.lastIndexOf("/");
    var parentDir = parentDirOf(pathForQuery);
    var fileName = slash <= 0 ? pathForQuery.replace(/^\//, "") : pathForQuery.slice(slash + 1);

    Promise.all([
      apiFetch("/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/tags/?start=0&limit=1000").then(function (r) { return r.json(); }),
      apiFetch("/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/record/?parent_dir=" + encodeURIComponent(parentDir) + "&name=" + encodeURIComponent(fileName) + "&file_name=" + encodeURIComponent(fileName)).then(function (r) { return r.json(); }),
    ]).then(function (results) {
      if (tagsReqId !== rpReqId || userMutated) return; // stale response
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
    }).catch(function () {
      if (tagsReqId !== rpReqId || userMutated) return; // stale response
      renderTagChips(); // fileTagIds is empty → shows the "no tags" hint
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
          p = apiFetch("/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/tags/", {
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
          if (Toast && Toast.error) Toast.error(__t('fb.add_tag_failed'));
        });
      };

      tagsList.onclick = function (e) {
        var rm = e.target.closest(".js-rp-tag-remove");
        if (!rm) return;
        var tid = decodeURIComponent(rm.dataset.tagId);
        var next = fileTagIds.filter(function (id) { return String(id) !== String(tid); });
        saveTags(next).catch(function () {
          if (Toast && Toast.error) Toast.error(__t('fb.remove_tag_failed'));
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
          if (reqId !== rpReqId) return; // stale response
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
                '<a href="' + escapeAttr(linkUrl) + '" target="_blank" class="text-xs text-emerald-500 hover:text-emerald-600 truncate block">' +
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
        if (reqId !== rpReqId) return; // stale response
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
        if (reqId !== rpReqId) return; // stale response
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

  // Upload-link button is only meaningful for directories.
  var uploadLinkBtn = document.getElementById("rp-upload-link-btn");
  if (uploadLinkBtn) {
    uploadLinkBtn.style.display = d.type === "dir" ? "" : "none";
  }
}

// ─── Multi-select right panel ───────────────────────────────────────────
export function openMultiSelectPanel(selectedItems) {
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
}

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
export function thumbFailed(img) {
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
}

// Reset right panel to placeholder state
export function resetRightPanel() {
  var ph = document.querySelector(".js-rp-placeholder");
  var ct = document.querySelector(".js-rp-content");
  var mc = document.querySelector(".js-rp-multi-content");
  // Stop any playing media so it doesn't keep buffering in the background.
  stopRightPanelMedia();
  if (ph) ph.classList.remove("hidden");
  if (ct) ct.classList.add("hidden");
  if (mc) mc.classList.add("hidden");
  var uploadLinkBtn = document.getElementById("rp-upload-link-btn");
  if (uploadLinkBtn) uploadLinkBtn.style.display = "none";
}

// ─── Quick preview modal (dblclick on a file row) ───────────────────────
var QUICK_PREVIEW_TEXT_LIMIT = 1024 * 1024; // 1MB

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

export function hideQuickPreview() {
  var overlay = document.getElementById("quick-preview-overlay");
  if (!overlay) return;
  resetQuickPreview();
  overlay.classList.add("hidden");
}

export function openQuickPreview(row) {
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

  var encPath = encodeFilePath(path);
  var title = overlay.querySelector(".js-qp-title");
  if (title) title.textContent = name;

  var img = overlay.querySelector(".js-qp-img");
  var video = overlay.querySelector(".js-qp-video");
  var audio = overlay.querySelector(".js-qp-audio");
  var text = overlay.querySelector(".js-qp-text");

  if (isVideo) {
    video.src = "/repos/" + encodeURIComponent(repoId) + "/files/" + encPath;
    video.classList.remove("hidden");
  } else if (isAudio) {
    audio.src = "/repos/" + encodeURIComponent(repoId) + "/files/" + encPath;
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
}

// Quick preview modal event bindings (close button, backdrop, ESC).
(function () {
  var overlay = document.getElementById("quick-preview-overlay");
  if (!overlay) return;
  var close = overlay.querySelector(".js-qp-close");
  if (close) close.addEventListener("click", function () { hideQuickPreview(); });
  overlay.addEventListener("click", function (e) {
    if (e.target === overlay) hideQuickPreview();
  });
})();

document.addEventListener("keydown", function (e) {
  if (e.key === "Escape") {
    var overlay = document.getElementById("quick-preview-overlay");
    if (overlay && !overlay.classList.contains("hidden")) hideQuickPreview();
  }
});

// ─── Helpers ────────────────────────────────────────────────────────────
function setText(container, selector, val) {
  var el = container.querySelector(selector);
  if (el) el.textContent = val;
}

// ─── Reindex single file ────────────────────────────────────────────────
document.addEventListener("click", async function (e) {
  var btn = e.target.closest(".js-rp-reindex-btn");
  if (!btn) return;
  var repoId = btn.dataset.repoId;
  var path = btn.dataset.path;
  if (!repoId || !path) return;
  try {
    btn.disabled = true;
    btn.textContent = "Indexing...";
    var resp = await apiFetch("/api2/repos/" + encodeURIComponent(repoId) + "/file/reindex/", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ p: path }),
    });
    var result = await resp.json();
    if (result.indexed) {
      Toast.success("Reindexed");
    } else {
      Toast.info("File type not supported for indexing");
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
  } catch (err) {
    Toast.error("Reindex failed: " + (err.message || err));
  } finally {
    btn.textContent = __t('ui.reindex');
    btn.disabled = false;
  }
});

// Initial check in case an item is pre-selected on load (upload-link button
// visibility depends on the selected entry type).
setTimeout(function () {
  var btn = document.getElementById("rp-upload-link-btn");
  if (!btn) return;
  var selected = document.querySelector(".selected[data-type]");
  var type = selected ? selected.getAttribute("data-type") : "";
  btn.style.display = type === "dir" ? "" : "none";
}, 100);

// Thumbnail error fallback — `error` events don't bubble, so capture at the
// document level. Matches the `<img data-thumb>` markers emitted by file_list
// and right_panel templates.
document.addEventListener("error", function (e) {
  var img = e.target;
  if (!img || img.tagName !== "IMG" || !img.hasAttribute("data-thumb")) return;
  thumbFailed(img);
}, true);
