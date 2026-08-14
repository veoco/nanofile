// pages — small page-scoped interactions that are shared across all pages
// (trash restore, repo list filter, new-library dialog). Kept in the common
// bundle because their triggering elements appear on non-file-browser pages.
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

// ─── New Library dialog ─────────────────────────────────────────────────
export function showQuickCreate() {
  var overlay = document.getElementById("quick-create-overlay");
  if (!overlay) return;
  overlay.classList.remove("hidden");
  var input = document.getElementById("quick-create-input");
  if (input) { input.value = ""; setTimeout(function () { input.focus(); }, 100); }
}

export function hideQuickCreate() {
  var overlay = document.getElementById("quick-create-overlay");
  if (overlay) overlay.classList.add("hidden");
}

export function submitQuickCreate() {
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
      else { r.json().then(function (e) { Toast.error(e.error_msg || __t('ui.failed')); }); }
    })
    .catch(function () { Toast.error(__t('ui.network_error')); });
  hideQuickCreate();
  return false;
}
