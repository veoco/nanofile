// pages — small page-scoped interactions that are shared across all pages
// (trash restore, repo list filter). Kept in the common bundle because their
// triggering elements appear on non-file-browser pages.
import { __t } from "./i18n.js";
import { getCookie } from "./utils.js";
import { Toast } from "./toast.js";
import { ConfirmDialog } from "./confirm.js";
import { showPreviewFailure } from "./preview-failure.js";

// ─── Trash restore (via API) ────────────────────────────────────────────
document.addEventListener("submit", async function (e) {
  const form = e.target.closest(".js-restore-form");
  if (!form) return;
  e.preventDefault();

  const repoId = form.querySelector('[name="repo_id"]').value;
  const commitId = form.querySelector('[name="commit_id"]').value;
  const path = form.querySelector('[name="path"]').value;
  const objName = form.dataset.objName || "";
  const repoName = form.dataset.repoName || "";

  var confirmed = await ConfirmDialog.confirm(
    __t('ui.restore'),
    __t('ui.confirm_restore', { name: objName, repo: repoName }),
    { confirmText: __t('ui.restore'), variant: "primary" }
  );
  if (!confirmed) return;

  // Build request body: { commit_id: [path] }
  var body = {};
  body[commitId] = [path];

  var csrfToken = getCookie("sfcsrftoken");
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
      Toast.error(__t('ui.restore_failed_short'));
    }
  } catch (err) {
    Toast.error(__t('ui.restore_failed', { msg: err.message }));
  }
});

// ─── Repo filter ────────────────────────────────────────────────────────
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

// ─── Generic confirm-before-submit ─────────────────────────────────────
// data-confirm="<i18n key>" plus optional data-confirm-args (JSON) for
// {placeholder} substitution, e.g. data-confirm-args='{"name":"x"}'.
// data-confirm-variant="solid" for a non-destructive action (default: danger).
// The submit is always cancelled up front; when the dialog says yes the form is
// submitted through the DOM API, which does not re-fire this listener.
document.addEventListener("submit", async function (e) {
    var form = e.target.closest("[data-confirm]");
    if (!form) return;
    e.preventDefault();
    var msg;
    if (form.dataset.confirmArgs) {
        msg = __t(form.dataset.confirm, JSON.parse(form.dataset.confirmArgs));
    } else {
        msg = __t(form.dataset.confirm);
    }
    var confirmed = await ConfirmDialog.confirm(
        __t('common.are_you_sure'),
        msg,
        {
            confirmText: __t('common.confirm'),
            variant: form.dataset.confirmVariant || 'danger',
        }
    );
    if (!confirmed) return;
    HTMLFormElement.prototype.submit.call(form);
});

// ─── Copy-to-clipboard (data-copy="<selector>") ────────────────────────
// For values that are too long to read at phone width but still have to be
// handed to someone, e.g. an invitation code: the button copies the target's
// text, which stays in the DOM, so the page degrades to select-and-copy when
// this module is absent. `data-copy-msg` names the toast to confirm with, and
// `data-copy-lines` on the target copies one line per child element, for a
// grid of separate values.
document.addEventListener("click", function (e) {
    var btn = e.target.closest("[data-copy]");
    if (!btn) return;
    var target = document.querySelector(btn.dataset.copy);
    if (!target) return;
    var text;
    if (target.hasAttribute("data-copy-lines")) {
        text = Array.prototype.map
            .call(target.children, function (child) { return child.textContent.trim(); })
            .filter(Boolean)
            .join("\n");
    } else {
        text = (target.textContent || "").trim();
    }
    if (!text) return;
    if (navigator.clipboard) navigator.clipboard.writeText(text).catch(function () {});
    if (btn.dataset.copyMsg) Toast.success(__t(btn.dataset.copyMsg));
});

// ─── Preview fallback (data-preview-image / data-preview-media) ────────
// `error` events don't bubble, so capture at the document level. A picture or a
// player the browser cannot decode is replaced by the same sentence — and the
// same download escape — the quick-preview dialog shows for that file. A media
// element that cannot decode otherwise just sits there as a black box with a
// spinner, which is what the full-page viewer used to do.
document.addEventListener("error", function (e) {
    var el = e.target;
    if (!el || !el.hasAttribute) return;
    var kind = el.hasAttribute("data-preview-image") ? "image"
        : el.hasAttribute("data-preview-media") ? "media" : null;
    if (!kind || !el.parentElement) return;
    // `MEDIA_ERR_ABORTED` is what a caller that re-points `src` or calls
    // `load()` produces; it is not the file failing to decode.
    if (kind === "media" && el.error && el.error.code === 1) return;
    showPreviewFailure(el.parentElement, kind, el.getAttribute("data-download-url") || "");
}, true);
