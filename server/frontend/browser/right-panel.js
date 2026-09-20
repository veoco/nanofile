// right-panel — the bottom details drawer: summary + rich tier for a single
// selection, a batch summary for a multi selection, and the quick-preview modal.
//
// The drawer replaced the old 300px right-hand column. It is an OVERLAY pinned
// to the bottom of the file-manager column, so opening it reflows nothing: the
// list keeps its exact height and scroll offset. The only thing that changes is
// `.nf-scroll`'s bottom padding, which reserves room so the final row can still
// be scrolled clear of the drawer.
import { __t } from "../core/i18n.js";
import { escapeHtml, escapeAttr, encodeFilePath, safeColor, parentDirOf } from "../core/utils.js";
import { apiFetch } from "../core/api.js";
import { humanType, isQuickPreviewImage, getExifFields } from "../core/file-meta.js";
import { formatLocalDateTime } from "../core/format.js";
import { Toast } from "../core/toast.js";
import { refreshFileList } from "./list.js";

// Monotonic id used to discard stale right-panel async responses when the
// selection changes faster than the detail requests (share/upload links,
// index text, EXIF, tags) resolve.
var rpReqId = 0;

// ─── Drawer plumbing ────────────────────────────────────────────────────
var detailsEl = document.getElementById("nf-details");
var contentEl = document.getElementById("nf-content");
var detailsExpandEl = document.getElementById("nf-details-expand");

var EXPAND_KEY = "nfDetailsExpanded";

function isExpanded() {
  try { return localStorage.getItem(EXPAND_KEY) === "true"; } catch (e) { return false; }
}

// The rich tier is remembered per browser: it is collapsed by default because
// the summary answers the common "what is this file" question, but a user who
// wants tags/EXIF/link lists every time should only have to say so once.
function setExpanded(expanded) {
  if (detailsExpandEl) detailsExpandEl.hidden = !expanded;
  document.querySelectorAll(".js-d-toggle").forEach(function (t) {
    t.setAttribute("aria-expanded", expanded ? "true" : "false");
  });
  try { localStorage.setItem(EXPAND_KEY, expanded ? "true" : "false"); } catch (e) { /* ignore */ }
}

function syncDrawerHeight() {
  if (!detailsEl || !contentEl) return;
  var h = detailsEl.offsetHeight;
  contentEl.style.setProperty("--drawer-h", h + "px");
  return h;
}

function openDrawer() {
  if (!detailsEl || !contentEl) return;
  setExpanded(isExpanded());
  // Measure before revealing: `visibility: hidden` still has layout, and the
  // reserved padding must match the drawer's real height.
  syncDrawerHeight();
  detailsEl.classList.add("open");
  contentEl.classList.add("drawer-open");
}

function closeDrawer() {
  if (!detailsEl || !contentEl) return;
  detailsEl.classList.remove("open");
  contentEl.classList.remove("drawer-open");
  contentEl.style.removeProperty("--drawer-h");
}

document.addEventListener("click", function (e) {
  if (!e.target.closest(".js-d-toggle")) return;
  if (!detailsExpandEl) return;
  setExpanded(detailsExpandEl.hidden);
  // The drawer just changed height, so the space reserved at the bottom of the
  // list must follow in the same frame — reading `offsetHeight` forces the
  // layout that `hidden` just invalidated, so this needs no rAF.
  if (detailsEl && detailsEl.classList.contains("open")) syncDrawerHeight();
});

// Escape clears the selection (which closes the drawer) unless the quick
// preview modal is the thing on top.
document.addEventListener("keydown", function (e) {
  if (e.key !== "Escape") return;
  if (!detailsEl || !detailsEl.classList.contains("open")) return;
  var qp = document.getElementById("quick-preview-overlay");
  if (qp && !qp.classList.contains("hidden")) return;
  var closeBtn = detailsEl.querySelector(".js-deselect-all");
  if (closeBtn) closeBtn.click();
});

export function openRightPanel(d) {
  // d = { name, type, starred, extension, path, repoId, modifierEmail, mtime,
  //       thumbnailUrl, thumbnailUrlLarge, isPreviewable, downloadUrl,
  //       isVideo, isAudio, sizeDisplay, recordId }

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
  var mediaSection = ct.querySelector(".js-rp-media");

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

  // The summary row only has room for a 54px poster, so playback lives in the
  // expandable tier; a file is "media" when that tier has something to show.
  var isMedia = false;

  if (d.type === "dir") {
    if (folderIcon) folderIcon.classList.remove("hidden");
  } else if (d.isVideo) {
    isMedia = true;
    // Inline playback via the Range-capable streaming endpoint; the frame
    // thumbnail (if any) doubles as the native poster and the summary image.
    if (videoEl && d.repoId && d.path) {
      videoEl.src = "/repos/" + encodeURIComponent(d.repoId) + "/files/" + encodeFilePath(d.path);
      videoEl.poster = d.thumbnailUrlLarge || d.thumbnailUrl || "";
      videoEl.classList.remove("hidden");
    }
    if (d.thumbnailUrlLarge || d.thumbnailUrl) {
      if (thumbImg) { thumbImg.dataset.extension = d.extension || ""; thumbImg.src = d.thumbnailUrlLarge || d.thumbnailUrl; thumbImg.classList.remove("hidden"); }
    } else if (videoIcon) {
      videoIcon.classList.remove("hidden");
    }
  } else if (d.isAudio) {
    isMedia = true;
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

  if (mediaSection) mediaSection.classList.toggle("hidden", !isMedia);

  // ── Basic Info ──
  setText(ct, ".js-rp-name", d.name || "");
  setText(ct, ".js-rp-type", humanType(d.type, d.extension));

  // ── Starred ──
  // The hook classes are never replaced: `openRightPanel` runs again for every
  // selection, and clobbering `className` used to make the button stop
  // updating from the second selection onward.
  var starBtn = ct.querySelector(".js-rp-star");
  if (starBtn) {
    var isStarred = d.starred === true || d.starred === "true";
    starBtn.dataset.starred = isStarred ? "true" : "false";
    starBtn.dataset.repoId = d.repoId || "";
    starBtn.dataset.path = d.path || "";
    starBtn.setAttribute("data-toggle-star", "");
    starBtn.classList.toggle("on", isStarred);
    var starIcon = ct.querySelector(".js-rp-star-icon");
    var starLabel = ct.querySelector(".js-rp-star-label");
    if (starIcon) starIcon.setAttribute("fill", isStarred ? "currentColor" : "none");
    if (starLabel) starLabel.textContent = isStarred ? __t('ui.starred') : __t('ui.not_starred');
    starBtn.title = isStarred ? __t('ui.unstar') : __t('ui.star');
  }

  // ── Details ──
  setText(ct, ".js-rp-path", d.path || "");
  setText(ct, ".js-rp-size", d.type === "dir" ? "—" : (d.sizeDisplay || ""));

  var mtime = parseInt(d.mtime, 10);
  setText(ct, ".js-rp-mtime", isNaN(mtime) ? "" : formatLocalDateTime(mtime));

  // ── Actions ──
  // Download
  var downloadLink = ct.querySelector(".js-rp-download");
  if (downloadLink) {
    downloadLink.href = d.type === "dir" ? "#" : (d.downloadUrl || "#");
    downloadLink.classList.toggle("pointer-events-none", d.type === "dir");
    downloadLink.classList.toggle("opacity-50", d.type === "dir");
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
      historyBtn.classList.add("inline-flex");
    } else {
      historyBtn.classList.add("hidden");
      historyBtn.classList.remove("inline-flex");
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
      shareList.innerHTML = '<div class="js-rp-share-links-loading text-[11px] text-ink-3 italic">' + escapeHtml(__t('fb.loading')) + '</div>';
      fetch("/api/v2.1/share-links/?repo_id=" + encodeURIComponent(d.repoId) + "&path=" + encodeURIComponent(d.path))
        .then(function (r) { return r.json(); })
        .then(function (data) {
          if (reqId !== rpReqId) return; // stale response
          var links = data || [];
          shareList.innerHTML = "";
          if (links.length === 0) {
            shareList.innerHTML = '<div class="js-rp-no-share-links text-[11px] text-ink-3 italic">' + escapeHtml(__t('fb.no_share_links')) + '</div>';
          } else {
            links.forEach(function (link) {
              var div = document.createElement("div");
              div.className = "flex items-center justify-between py-0.5";
              div.innerHTML =
                '<a href="' + escapeAttr(link.link || "") + '" target="_blank" class="text-[11px] text-ink truncate block hover:underline">' +
                  escapeHtml(link.token || "") +
                '</a>' +
                '<span class="mono flex-shrink-0 ml-2">' + (link.view_cnt || 0) + ' views</span>';
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
          '<span class="js-rp-no-tags text-[11px] text-ink-3 italic">' +
          escapeHtml(__t('fb.no_tags')) +
          "</span>";
        return;
      }
      fileTagIds.forEach(function (tid) {
        var tag = allTags.find(function (t) { return String(t.id) === String(tid); });
        if (!tag) return;
        var chip = document.createElement("span");
        chip.className = "js-rp-tag-chip inline-flex items-center gap-1 rounded-[4px] px-1.5 py-0.5 text-[10px] font-medium text-ink";
        var tagColor = safeColor(tag.color);
        chip.style.backgroundColor = tagColor + "33";
        chip.innerHTML =
          '<span class="inline-block h-1.5 w-1.5 rounded-full" style="background-color:' + escapeAttr(tagColor) + ';"></span>' +
          escapeHtml(tag.name) +
          '<button type="button" class="js-rp-tag-remove hover:text-err" data-tag-id="' + encodeURIComponent(tag.id) + '" title="' + escapeAttr(__t('fb.remove_tag')) + '">' +
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
          var colors = ["#e5484d", "#e54d2e", "#e8890c", "#c99a2e", "#46a758", "#12a594", "#0090ff", "#8e4ec6", "#e93d82", "#8f8f8f"];
          var color = colors[Math.floor(Math.random() * colors.length)];
          p = apiFetch("/api/v2.1/repos/" + encodeURIComponent(repoId) + "/metadata/tags/", {
            method: "POST",
            body: JSON.stringify({ tags_data: [{ _tag_name: name, _tag_color: color }] }),
          }).then(function (r) { return r.json(); }).then(function (data) {
            var created = (data.tags || [])[0];
            // Mark the panel as user-mutated so the initial tags/record load
            // (which may still be in flight and reflects pre-save state) is
            // discarded instead of overwriting the tag we just created.
            userMutated = true;
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
                '<a href="' + escapeAttr(linkUrl) + '" target="_blank" class="text-[11px] text-ink truncate block hover:underline">' +
                  escapeHtml(link.token || "") +
                '</a>' +
                '<span class="mono flex-shrink-0 ml-2">' + (link.view_cnt || 0) + ' uploads</span>';
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
            div.className = "flex items-center justify-between gap-3";
            div.innerHTML = '<span class="text-[11px] text-ink-3">' + f.label + '</span>' +
              '<span class="text-[11px] font-medium text-ink text-right">' + escapeHtml(f.value) + '</span>';
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
    uploadLinkBtn.classList.toggle("hidden", d.type !== "dir");
  }

  openDrawer();
}

// ─── Multi-select summary ───────────────────────────────────────────────
export function openMultiSelectPanel(selectedItems) {
  // selectedItems = [{ name, type }, ...]
  var ph = document.querySelector(".js-rp-placeholder");
  var ct = document.querySelector(".js-rp-content");
  var mc = document.querySelector(".js-rp-multi-content");
  if (!ph || !ct || !mc) return;

  // The drawer is no longer replaced by this function, so any in-flight
  // single-selection responses must be invalidated here too.
  rpReqId++;

  ph.classList.add("hidden");
  ct.classList.add("hidden");
  mc.classList.remove("hidden");

  var countEl = mc.querySelector(".js-rp-multi-count");
  if (countEl) {
    countEl.textContent = selectedItems.length + " " + __t('fb.selected');
  }

  // Only a handful of names fit on one line; the list would otherwise push the
  // buttons off the row for a large selection.
  var listEl = mc.querySelector(".js-rp-multi-list");
  if (listEl) {
    var names = selectedItems.slice(0, 4).map(function (item) {
      return item.name + (item.type === "dir" ? "/" : "");
    });
    var rest = selectedItems.length - names.length;
    listEl.textContent = names.join(", ") + (rest > 0 ? " +" + rest : "");
  }

  openDrawer();
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
  // Drawer thumbnails (e.g. audio without cover art): fall back to the
  // extension badge, same as unknown files.
  var extBadge = document.querySelector(".js-rp-content .js-rp-ext-badge");
  if (extBadge) {
    extBadge.textContent = img.dataset && img.dataset.extension ? img.dataset.extension : "?";
    extBadge.classList.remove("hidden");
  }
}

// Reset the drawer to its closed state
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
  if (uploadLinkBtn) uploadLinkBtn.classList.add("hidden");
  // Invalidate in-flight detail requests for the selection being dismissed.
  rpReqId++;
  closeDrawer();
}

// ─── Quick preview modal (dblclick on a file row) ───────────────────────
var QUICK_PREVIEW_TEXT_LIMIT = 1024 * 1024; // 1MB

function showQuickPreviewUnsupported(messageKey) {
  var overlay = document.getElementById("quick-preview-overlay");
  if (!overlay) return;
  var unsupported = overlay.querySelector(".js-qp-unsupported");
  if (unsupported) {
    unsupported.textContent = __t(messageKey || "fb.preview_failed");
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
  if (video) { video.pause(); video.removeAttribute("src"); video.load(); video.onerror = null; }
  if (audio) { audio.pause(); audio.removeAttribute("src"); audio.load(); audio.onerror = null; }
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
    // A media element that cannot decode the file fires `error` and just sits
    // there, so without this the dialog was an empty black box. Which files
    // decode is the browser's codec list, not the container: an HEVC/H.265 clip
    // in the same .mp4/.MOV the phone also writes H.264 into plays nowhere on a
    // stock Windows Chrome/Edge.
    video.onerror = function () {
      video.classList.add("hidden");
      showQuickPreviewUnsupported("fb.media_unsupported");
    };
    video.src = "/repos/" + encodeURIComponent(repoId) + "/files/" + encPath;
    video.classList.remove("hidden");
  } else if (isAudio) {
    audio.onerror = function () {
      audio.classList.add("hidden");
      showQuickPreviewUnsupported("fb.media_unsupported");
    };
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
    btn.textContent = __t('ui.indexing');
    var resp = await apiFetch("/api2/repos/" + encodeURIComponent(repoId) + "/file/reindex/", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ p: path }),
    });
    var result = await resp.json();
    if (result.indexed) {
      Toast.success(__t('ui.reindexed'));
    } else {
      Toast.info(__t('ui.reindex_unsupported'));
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
    Toast.error(__t('ui.reindex_failed', { msg: err.message || err }));
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
  btn.classList.toggle("hidden", type !== "dir");
}, 100);

// Thumbnail error fallback — `error` events don't bubble, so capture at the
// document level. Matches the `<img data-thumb>` markers emitted by file_list
// and details_drawer templates.
document.addEventListener("error", function (e) {
  var img = e.target;
  if (!img || img.tagName !== "IMG" || !img.hasAttribute("data-thumb")) return;
  thumbFailed(img);
}, true);
