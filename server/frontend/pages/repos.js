// repos — library list page interactions (create/edit/delete library and
// WebDAV key management). Migrated from the inline <script> block that used to
// live in templates/repos/list.html; now driven by data-* event delegation.
import { __t } from "../core/i18n.js";
import { getCookie } from "../core/utils.js";
import { registerModalClose } from "../core/modal.js";

var reposPageEl = document.getElementById("repos-page");
var webdavBaseUrl = reposPageEl ? (reposPageEl.dataset.webdavBaseUrl || "") : "";

function showCreateDialog() {
    document.getElementById('create-overlay').classList.remove('hidden');
    document.getElementById('create-input').value = '';
    setTimeout(function () { document.getElementById('create-input').focus(); }, 100);
}
function hideCreateDialog() {
    document.getElementById('create-overlay').classList.add('hidden');
}
function submitCreate(form) {
    var name = document.getElementById('create-input').value.trim();
    if (!name) return false;
    hideCreateDialog();
    var csrfToken = form.querySelector('[name="csrf_token"]').value || '';
    fetch('/api2/repos/', {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
            'Content-Type': 'application/json;charset=utf-8',
            'X-CSRFToken': csrfToken,
        },
        body: JSON.stringify({ name: name }),
    }).then(function (resp) {
        if (resp.ok) {
            window.location.reload();
        } else {
            alert(__t('repo.create_failed') + resp.status);
        }
    }).catch(function () {
        alert(__t('common.network_error'));
    });
    return false;
}

function showEditDialog(btn) {
    var repoId = btn.getAttribute('data-id');
    document.getElementById('edit-repo-id').value = repoId;
    document.getElementById('edit-name').value = btn.getAttribute('data-name');
    document.getElementById('edit-description').value = btn.getAttribute('data-description') || '';
    document.getElementById('edit-size').textContent = btn.getAttribute('data-size');
    document.getElementById('edit-history-limit').value = btn.getAttribute('data-history-limit') || '0';
    document.getElementById('edit-history-ttl-days').value = btn.getAttribute('data-history-ttl-days') || '0';
    document.getElementById('webdav-url').textContent = webdavBaseUrl + '/dav/' + repoId + '/';
    document.getElementById('edit-overlay').classList.remove('hidden');
    setTimeout(function () { document.getElementById('edit-name').focus(); document.getElementById('edit-name').select(); }, 100);
}
function hideEditDialog() {
    document.getElementById('edit-overlay').classList.add('hidden');
}
function copyWebdavUrl() {
    copyToClipboard(document.getElementById('webdav-url').textContent, 'copy-url-btn');
}
function copyToClipboard(text, btnId) {
    navigator.clipboard.writeText(text).catch(function () {});
    flashCopiedBtn(btnId);
}
function flashCopiedBtn(id) {
    var btn = document.getElementById(id);
    if (btn._copyTimer) { clearTimeout(btn._copyTimer); }
    var orig = btn.textContent;
    btn.textContent = __t('webdav.copied');
    btn._copyTimer = setTimeout(function () { btn.textContent = orig; btn._copyTimer = null; }, 1500);
}
function submitEdit(form) {
    var repoId = document.getElementById('edit-repo-id').value;
    var newName = document.getElementById('edit-name').value.trim();
    var newDesc = document.getElementById('edit-description').value.trim();
    if (!newName) { alert(__t('repo.name_empty')); return false; }
    hideEditDialog();
    var csrfToken = form.querySelector('[name="csrf_token"]').value || '';
    var historyLimit = numOrNull('edit-history-limit');
    var historyTtlDays = numOrNull('edit-history-ttl-days');
    fetch('/api2/repos/' + repoId + '/?op=update', {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
            'Content-Type': 'application/json;charset=utf-8',
            'X-CSRFToken': csrfToken,
        },
        body: JSON.stringify({
            repo_name: newName,
            description: newDesc,
            history_limit: historyLimit,
            history_ttl_days: historyTtlDays,
        }),
    }).then(function (resp) {
        if (resp.ok) {
            window.location.reload();
        } else {
            alert(__t('repo.update_failed') + resp.status);
        }
    }).catch(function () {
        alert(__t('common.network_error'));
    });
    return false;
}

function deleteRepo(btn) {
    var repoId = btn.getAttribute('data-repo-id');
    var repoName = btn.getAttribute('data-repo-name');
    var csrfToken = getCookie('sfcsrftoken');
    if (!csrfToken) { window.location.href = '/accounts/login/'; return; }

    if (!confirm(__t('repo.delete_confirm', { name: repoName }))) return;

    fetch('/api2/repos/' + repoId + '/', {
        method: 'DELETE',
        credentials: 'same-origin',
        headers: { 'X-CSRFToken': csrfToken },
    }).then(function (resp) {
        if (resp.ok) {
            window.location.reload();
        } else {
            alert(__t('repo.delete_failed') + resp.status);
        }
    }).catch(function () {
        alert(__t('common.network_error'));
    });
}

// Read a numeric input; empty/invalid → null so the server leaves it unchanged.
function numOrNull(id) {
    var v = document.getElementById(id).value.trim();
    if (v === '' || isNaN(Number(v))) return null;
    return Number(v);
}

// ─── Event delegation ───────────────────────────────────────────────────
document.addEventListener("click", function (e) {
    var el = e.target.closest("[data-action]");
    if (!el) return;
    var action = el.dataset.action;
    switch (action) {
        case "show-create": showCreateDialog(); break;
        case "show-edit": showEditDialog(el); break;
        case "delete-repo": deleteRepo(el); break;
        case "copy-webdav-url": copyWebdavUrl(); break;
        case "close-create": hideCreateDialog(); break;
        case "close-edit": hideEditDialog(); break;
    }
});

document.addEventListener("submit", function (e) {
    var createForm = e.target.closest('[data-form="create"]');
    if (createForm) { e.preventDefault(); submitCreate(createForm); return; }
    var editForm = e.target.closest('[data-form="edit"]');
    if (editForm) { e.preventDefault(); submitEdit(editForm); return; }
});

document.addEventListener("keydown", function (e) {
    var createInput = e.target.closest("#create-input");
    if (createInput) {
        if (e.key === "Escape") { e.preventDefault(); hideCreateDialog(); }
        else if (e.key === "Enter") { e.preventDefault(); createInput.form.querySelector('button[type="submit"]').click(); }
        return;
    }
    if (e.target.closest("#edit-name")) {
        if (e.key === "Escape") { e.preventDefault(); hideEditDialog(); }
        return;
    }
});

registerModalClose("hideCreateDialog", hideCreateDialog);
registerModalClose("hideEditDialog", hideEditDialog);
