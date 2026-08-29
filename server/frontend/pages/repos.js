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
    document.getElementById('webdav-key-name').value = '';
    dismissNewKey();
    loadWebdavKeys(repoId);
    document.getElementById('edit-overlay').classList.remove('hidden');
    setTimeout(function () { document.getElementById('edit-name').focus(); document.getElementById('edit-name').select(); }, 100);
}
function hideEditDialog() {
    document.getElementById('edit-overlay').classList.add('hidden');
}
function loadWebdavKeys(repoId) {
    var listEl = document.getElementById('webdav-key-list');
    listEl.innerHTML = '<li class="text-sm text-gray-400 dark:text-gray-500">' + __t('webdav.loading') + '</li>';
    fetch('/api2/repos/' + repoId + '/webdav-keys/', {
        method: 'GET',
        credentials: 'same-origin',
    }).then(function (resp) {
        if (!resp.ok) {
            listEl.innerHTML = '<li class="text-sm text-gray-400 dark:text-gray-500">' + __t('webdav.load_keys_failed') + '</li>';
            return null;
        }
        return resp.json();
    }).then(function (data) {
        if (data === null) return;
        if (!data.keys || data.keys.length === 0) {
            listEl.innerHTML = '<li class="text-sm text-gray-400 dark:text-gray-500">' + __t('webdav.no_keys') + '</li>';
            return;
        }
        listEl.innerHTML = '';
        data.keys.forEach(function (k) {
            var li = document.createElement('li');
            li.className = 'flex items-center justify-between gap-2 py-1';
            var left = document.createElement('div');
            left.className = 'flex items-center min-w-0 gap-2';
            var icon = document.createElement('span');
            icon.className = 'flex h-6 w-6 flex-shrink-0 items-center justify-center rounded-md bg-gray-100 dark:bg-surface-700 text-gray-400 dark:text-gray-500';
            icon.innerHTML = '<svg class="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">'
                + '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" '
                + 'd="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z"/></svg>';
            var text = document.createElement('div');
            text.className = 'min-w-0';
            var name = document.createElement('p');
            name.className = 'truncate text-sm font-medium text-gray-900 dark:text-gray-100';
            name.textContent = k.name;
            var meta = document.createElement('p');
            meta.className = 'text-xs text-gray-500 dark:text-gray-400';
            var badge = document.createElement('span');
            badge.className = 'mr-1 inline-block rounded px-1 py-0.5 text-[10px] font-medium ' +
                (k.permission === 'r'
                    ? 'bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-400'
                    : 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-400');
            badge.textContent = k.permission === 'r'
                ? __t('webdav.permission_r')
                : __t('webdav.permission_rw');
            meta.appendChild(badge);
            var firstUsed = new Date(k.created_at * 1000).toLocaleDateString();
            meta.appendChild(document.createTextNode(__t('webdav.created_at', { date: firstUsed })));
            if (k.last_used_at) {
                meta.appendChild(document.createTextNode(' · ' + __t('webdav.last_used_at', { date: new Date(k.last_used_at * 1000).toLocaleDateString() })));
            } else {
                meta.appendChild(document.createTextNode(' · ' + __t('webdav.never_used')));
            }
            text.appendChild(name);
            text.appendChild(meta);
            left.appendChild(icon);
            left.appendChild(text);
            var del = document.createElement('button');
            del.type = 'button';
            del.className = 'flex-shrink-0 text-sm font-medium text-red-500 hover:text-red-700';
            del.textContent = __t('common.delete');
            del.dataset.action = 'delete-webdav-key';
            del.dataset.repoId = repoId;
            del.dataset.keyId = String(k.id);
            li.appendChild(left);
            li.appendChild(del);
            listEl.appendChild(li);
        });
    }).catch(function () {
        listEl.innerHTML = '';
    });
}
function createWebdavKey() {
    var repoId = document.getElementById('edit-repo-id').value;
    if (!repoId) return;
    var name = document.getElementById('webdav-key-name').value.trim() || 'default';
    var permission = document.getElementById('webdav-key-permission').value || 'rw';
    var csrfToken = getCookie('sfcsrftoken');
    fetch('/api2/repos/' + repoId + '/webdav-keys/', {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
            'Content-Type': 'application/json;charset=utf-8',
            'X-CSRFToken': csrfToken,
        },
        body: JSON.stringify({ name: name, permission: permission }),
    }).then(function (resp) {
        if (!resp.ok) {
            alert(__t('webdav.generate_failed') + resp.status);
            return;
        }
        return resp.json();
    }).then(function (data) {
        if (!data) return;
        document.getElementById('new-key-value').value = data.key;
        document.getElementById('new-key-box').classList.remove('hidden');
        document.getElementById('webdav-key-name').value = '';
        loadWebdavKeys(repoId);
    }).catch(function () {
        alert(__t('common.network_error'));
    });
}
function copyWebdavUrl() {
    copyToClipboard(document.getElementById('webdav-url').textContent, 'copy-url-btn');
}
function copyNewKey() {
    var input = document.getElementById('new-key-value');
    input.select();
    input.setSelectionRange(0, input.value.length);
    copyToClipboard(input.value, 'copy-new-key-btn');
}
function copyToClipboard(text, btnId) {
    navigator.clipboard.writeText(text).catch(function () {});
    flashCopiedBtn(btnId);
}
function dismissNewKey() {
    document.getElementById('new-key-box').classList.add('hidden');
    document.getElementById('new-key-value').value = '';
}
function flashCopiedBtn(id) {
    var btn = document.getElementById(id);
    if (btn._copyTimer) { clearTimeout(btn._copyTimer); }
    var orig = btn.textContent;
    btn.textContent = __t('webdav.copied');
    btn._copyTimer = setTimeout(function () { btn.textContent = orig; btn._copyTimer = null; }, 1500);
}
function deleteWebdavKey(repoId, keyId) {
    if (!confirm(__t('webdav.delete_key_confirm'))) return;
    var csrfToken = getCookie('sfcsrftoken');
    fetch('/api2/repos/' + repoId + '/webdav-keys/' + keyId + '/', {
        method: 'DELETE',
        credentials: 'same-origin',
        headers: { 'X-CSRFToken': csrfToken },
    }).then(function (resp) {
        if (resp.ok) {
            loadWebdavKeys(repoId);
        } else {
            alert(__t('webdav.delete_failed') + resp.status);
        }
    }).catch(function () {
        alert(__t('common.network_error'));
    });
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
        case "copy-new-key": copyNewKey(); break;
        case "dismiss-new-key": dismissNewKey(); break;
        case "create-webdav-key": createWebdavKey(); break;
        case "close-create": hideCreateDialog(); break;
        case "close-edit": hideEditDialog(); break;
        case "delete-webdav-key": deleteWebdavKey(el.dataset.repoId, el.dataset.keyId); break;
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
    if (e.target.closest("#webdav-key-name")) {
        if (e.key === "Enter") { e.preventDefault(); createWebdavKey(); }
        return;
    }
});

registerModalClose("hideCreateDialog", hideCreateDialog);
registerModalClose("hideEditDialog", hideEditDialog);
