// upload-link-dialog — create/delete upload links for directories (the dialog
// in browser.html), plus the share/upload link URL copy helpers.
import { __t } from "../core/i18n.js";
import { apiFetch } from "../core/api.js";
import { Toast } from "../core/toast.js";

// ─── Upload Link Dialog ─────────────────────────────────────────────────
// Module-scoped reference, assigned by the IIFE below so delegated handlers
// (data-action="open-upload-link") can open the dialog without a global.
var openUploadLinkDialog;

(function () {
  var overlay = document.getElementById('upload-link-dialog-overlay');
  var pathDisplay = document.querySelector('.js-ul-dialog-path');
  var errorDiv = document.querySelector('.js-ul-dialog-error');
  var createBtn = document.getElementById('ul-create-btn');
  var deleteBtn = document.getElementById('ul-delete-btn');
  var cancelBtn = document.querySelector('.js-ul-cancel');
  var createForm = document.getElementById('ul-create-form');
  var linkDisplay = document.getElementById('ul-link-display');
  var linkUrl = document.getElementById('ul-link-url');
  var pwdInput = document.getElementById('ul-password-input');
  var expirySelect = document.getElementById('ul-expiry-select');
  var descInput = document.getElementById('ul-description-input');

  var currentRepoId = '';
  var currentPath = '';
  var currentToken = '';

  function open(repoId, path) {
    currentRepoId = repoId;
    currentPath = path;
    currentToken = '';
    pathDisplay.textContent = path;
    createForm.classList.remove('hidden');
    linkDisplay.classList.add('hidden');
    deleteBtn.classList.add('hidden');
    cancelBtn.classList.remove('hidden');
    createBtn.textContent = __t('common.create');
    createBtn.disabled = false;
    pwdInput.value = '';
    expirySelect.value = '';
    descInput.value = '';
    errorDiv.classList.add('hidden');
    overlay.classList.remove('hidden');
  }

  function showLink(url) {
    createForm.classList.add('hidden');
    linkDisplay.classList.remove('hidden');
    linkUrl.value = window.location.origin + url;
    deleteBtn.classList.remove('hidden');
    cancelBtn.classList.add('hidden');
    createBtn.textContent = __t('common.close');
  }

  createBtn.addEventListener('click', async function () {
    if (createBtn.textContent === __t('common.close')) {
      overlay.classList.add('hidden');
      return;
    }

    var body = { repo_id: currentRepoId, path: currentPath };
    var pwd = pwdInput.value.trim();
    if (pwd) body.password = pwd;
    var expiry = expirySelect.value;
    if (expiry) body.expire_days = parseInt(expiry, 10);
    var desc = descInput.value.trim();
    if (desc) body.description = desc;

    createBtn.disabled = true;
    createBtn.textContent = __t('fb.creating');
    errorDiv.classList.add('hidden');

    try {
      var resp = await apiFetch('/api/v2.1/upload-links/', {
        method: 'POST',
        body: JSON.stringify(body),
      });
      if (resp.ok) {
        var data = await resp.json();
        currentToken = data.token;
        showLink('/u/' + data.token + '/');
      } else {
        var text = await resp.text().catch(function () { return ''; });
        errorDiv.textContent = text || __t('fb.failed_create_upload_link');
        errorDiv.classList.remove('hidden');
      }
    } catch (err) {
      errorDiv.textContent = err.message;
      errorDiv.classList.remove('hidden');
    } finally {
      createBtn.disabled = false;
      createBtn.textContent = currentToken ? __t('common.close') : __t('common.create');
    }
  });

  deleteBtn.addEventListener('click', async function () {
    if (!currentToken) return;
    if (!confirm(__t('fb.confirm_delete_upload_link'))) return;

    deleteBtn.disabled = true;
    errorDiv.classList.add('hidden');

    try {
      var resp = await apiFetch('/api/v2.1/upload-links/' + currentToken + '/', {
        method: 'DELETE',
      });
      if (resp.ok) {
        currentToken = '';
        createForm.classList.remove('hidden');
        linkDisplay.classList.add('hidden');
        deleteBtn.classList.add('hidden');
        cancelBtn.classList.remove('hidden');
        createBtn.textContent = __t('common.create');
      } else {
        var text = await resp.text().catch(function () { return ''; });
        errorDiv.textContent = text || __t('fb.failed_delete_upload_link');
        errorDiv.classList.remove('hidden');
      }
    } catch (err) {
      errorDiv.textContent = err.message;
      errorDiv.classList.remove('hidden');
    } finally {
      deleteBtn.disabled = false;
    }
  });

  cancelBtn.addEventListener('click', function () { overlay.classList.add('hidden'); });
  overlay.addEventListener('click', function (e) { if (e.target === overlay) overlay.classList.add('hidden'); });

  openUploadLinkDialog = open;
})();

export function copyUploadLinkUrl() {
  var input = document.getElementById('ul-link-url');
  input.select();
  input.setSelectionRange(0, 99999);
  navigator.clipboard.writeText(input.value).catch(function () {});
}

export function copyShareLinkUrl() {
  var input = document.getElementById('share-link-url');
  input.select();
  input.setSelectionRange(0, 99999);
  navigator.clipboard.writeText(input.value).catch(function () {});
}

// Upload Link button in right panel — only for directories
document.addEventListener('click', function (e) {
  var btn = e.target.closest('#rp-upload-link-btn');
  if (!btn) return;
  var meta = document.querySelector('meta[name="repo-id"]');
  var repoId = meta ? meta.getAttribute('content') : '';
  // Find selected file entry
  var selectedRow = document.querySelector('.selected[data-type]');
  var path = selectedRow ? selectedRow.getAttribute('data-path') : '';
  var type = selectedRow ? selectedRow.getAttribute('data-type') : '';
  if (!repoId || !path) return;
  // Only allow upload links for directories
  if (type !== 'dir') {
    Toast.error(__t('fb.upload_links_dir_only'));
    return;
  }
  openUploadLinkDialog(repoId, path);
});

// Delegated handlers for the file-list upload-link button and the
// share/upload link URL copy buttons in browser.html.
document.addEventListener("click", function (e) {
  var el = e.target.closest("[data-action]");
  if (!el) return;
  var action = el.dataset.action;
  if (action === "open-upload-link") {
    openUploadLinkDialog(el.dataset.repoId, el.dataset.path);
  } else if (action === "copy-share-link") {
    copyShareLinkUrl();
  } else if (action === "copy-upload-link") {
    copyUploadLinkUrl();
  }
});
