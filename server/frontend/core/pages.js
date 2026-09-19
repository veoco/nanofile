// pages — small page-scoped interactions that are shared across all pages
// (trash restore, repo list filter). Kept in the common bundle because their
// triggering elements appear on non-file-browser pages.
import { __t } from "./i18n.js";
import { getCookie } from "./utils.js";
import { Toast } from "./toast.js";
import { ConfirmDialog } from "./confirm.js";

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

// ─── Preview image fallback (data-preview-image) ───────────────────────
// `error` events don't bubble, so capture at the document level. Builds the
// fallback with DOM APIs (not innerHTML) to keep the download URL inert.
document.addEventListener("error", function (e) {
    var img = e.target;
    if (!img || img.tagName !== "IMG" || !img.hasAttribute("data-preview-image")) return;
    var parent = img.parentElement;
    if (!parent) return;
    parent.innerHTML = '';
    var div = document.createElement('div');
    // Tokens, not the legacy `gray-*`/`brand-*` names the rest of this file
    // predates: the fallback renders inside the preview panel.
    div.className = 'text-ink-3 text-[13px] py-8 text-center';
    div.appendChild(document.createTextNode(__t('fb.image_failed_to_load') + ' '));
    var a = document.createElement('a');
    a.href = img.dataset.downloadUrl || '#';
    a.className = 'text-ink underline underline-offset-2';
    a.textContent = __t('fb.download_instead');
    div.appendChild(a);
    parent.appendChild(div);
}, true);
