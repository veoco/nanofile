// pages — page-scoped interactions shared across all pages (trash restore, repo list filter),
// in the common bundle because their triggers appear outside the file browser.
import { __t } from "./i18n.js";
import { getCookie } from "./utils.js";
import { Toast } from "./toast.js";
import { ConfirmDialog } from "./confirm.js";
import { showPreviewFailure } from "./preview-failure.js";

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

var repoFilter = document.querySelector(".js-repo-filter");
if (repoFilter) {
// Debounce so a fast typist is not re-filtering a large repo list on every keystroke.
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

// `data-confirm` names the i18n key, optional `data-confirm-args` (JSON) substitutes placeholders,
// and `data-confirm-variant="solid"` marks a non-destructive action. The submit is cancelled up
// front; when the dialog agrees the form is submitted through the DOM API, which does not refire.
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

// Copies a value too long to read at phone width. The target's text stays in the DOM, so the
// page degrades to select-and-copy, and `data-copy-lines` copies one line per child.
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

// `error` does not bubble, so capture at the document level. A picture or player that cannot
// decode gets the same sentence and download escape the quick-preview dialog shows.
document.addEventListener("error", function (e) {
    var el = e.target;
    if (!el || !el.hasAttribute) return;
    var kind = el.hasAttribute("data-preview-image") ? "image"
        : el.hasAttribute("data-preview-media") ? "media" : null;
    if (!kind || !el.parentElement) return;
// `MEDIA_ERR_ABORTED` comes from re-pointing `src` or calling `load()`, not a decode failure.
    if (kind === "media" && el.error && el.error.code === 1) return;
    showPreviewFailure(el.parentElement, kind, el.getAttribute("data-download-url") || "");
}, true);
