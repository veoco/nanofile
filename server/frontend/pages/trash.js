// trash — tab switching (deleted files / deleted libraries), emptying the file
// trash of one library, and restoring or permanently deleting a whole library.
import { __t } from "../core/i18n.js";
import { getCookie } from "../core/utils.js";
import { registerModalClose } from "../core/modal.js";
import { switchTab } from "../core/tabs.js";
import { ConfirmDialog } from "../core/confirm.js";
import { Toast } from "../core/toast.js";

function showCleanDialog() {
    document.getElementById('clean-dialog').classList.remove('hidden');
}
function hideCleanDialog() {
    document.getElementById('clean-dialog').classList.add('hidden');
}
function cleanTrash() {
    var select = document.getElementById('clean-repo');
    var repoId = select ? select.value : '';
    if (!repoId) { alert(__t('trash.select_library')); return; }
    if (!confirm(__t('trash.delete_all_confirm'))) return;
    var csrfToken = getCookie('sfcsrftoken');
    if (!csrfToken) {
        window.location.href = '/accounts/login/';
        return;
    }
    fetch('/api/v2.1/repos/' + encodeURIComponent(repoId) + '/trash/', {
        method: 'DELETE',
        credentials: 'same-origin',
        headers: { 'X-CSRFToken': csrfToken },
    }).then(function (r) {
        if (r.ok) window.location.reload();
        else alert(__t('trash.clean_failed'));
    }).catch(function () {
        alert(__t('common.network_error'));
    });
}

// ─── Deleted libraries ──────────────────────────────────────────────────

/**
 * Call the deleted-repos API, then reload the libraries tab with a status flag.
 *
 * The restore posts a form body, which is what seahub sends; the deletes carry
 * no body. Both need the CSRF header because the request rides the session
 * cookie.
 */
async function deletedReposRequest(method, path, body, flag) {
    var csrfToken = getCookie('sfcsrftoken');
    if (!csrfToken) {
        window.location.href = '/accounts/login/';
        return;
    }
    var opts = {
        method: method,
        credentials: 'same-origin',
        headers: { 'X-CSRFToken': csrfToken },
    };
    if (body) {
        opts.headers['Content-Type'] = 'application/x-www-form-urlencoded';
        opts.body = body;
    }
    try {
        var resp = await fetch(path, opts);
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        window.location.href = '/trash/?tab=libraries&' + flag + '=true';
    } catch (err) {
        Toast.error(__t('trash.lib_failed'));
    }
}

/** Restore one library with its files and history. */
async function restoreLibrary(el) {
    var name = el.dataset.repoName || '';
    var confirmed = await ConfirmDialog.confirm(
        __t('trash.restore'),
        __t('trash.lib_restore_confirm', { name: name }),
        { confirmText: __t('trash.restore'), variant: 'primary' }
    );
    if (!confirmed) return;
    await deletedReposRequest(
        'POST',
        '/api/v2.1/deleted-repos/',
        'repo_id=' + encodeURIComponent(el.dataset.repoId || ''),
        'lib_restored'
    );
}

/** Permanently delete one library and free its blocks. */
async function deleteLibrary(el) {
    var name = el.dataset.repoName || '';
    var confirmed = await ConfirmDialog.confirm(
        __t('trash.lib_delete_title'),
        __t('trash.lib_delete_confirm', { name: name }),
        { confirmText: __t('trash.lib_delete'), variant: 'danger' }
    );
    if (!confirmed) return;
    await deletedReposRequest(
        'DELETE',
        '/api/v2.1/deleted-repos/' + encodeURIComponent(el.dataset.repoId || '') + '/',
        null,
        'lib_deleted'
    );
}

/** Permanently delete every library in the trash. */
async function deleteAllLibraries() {
    var rows = document.querySelectorAll('#tab-libraries tbody tr').length;
    var confirmed = await ConfirmDialog.confirm(
        __t('trash.lib_delete_all'),
        __t('trash.lib_delete_all_confirm', { n: rows }),
        { confirmText: __t('trash.lib_delete_all'), variant: 'danger' }
    );
    if (!confirmed) return;
    await deletedReposRequest('DELETE', '/api/v2.1/deleted-repos/', null, 'libs_deleted');
}

document.addEventListener("click", function (e) {
    var el = e.target.closest("[data-action]");
    if (!el) return;
    switch (el.dataset.action) {
        case "tab":
            switchTab(el.dataset.tab, "files");
            break;
        case "open-clean-trash":
            showCleanDialog();
            break;
        case "close-clean":
            hideCleanDialog();
            break;
        case "clean-trash":
            cleanTrash();
            break;
        case "restore-lib":
            restoreLibrary(el);
            break;
        case "delete-lib":
            deleteLibrary(el);
            break;
        case "delete-all-libs":
            deleteAllLibraries();
            break;
    }
});

registerModalClose("hideCleanDialog", hideCleanDialog);
