// trash — tab switching (deleted files / deleted libraries), emptying the file
// trash of one library, and restoring or permanently deleting a whole library.
import { __t } from "../core/i18n.js";
import { mutate } from "../core/api.js";
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
async function cleanTrash() {
    var select = document.getElementById('clean-repo');
    var repoId = select ? select.value : '';
    if (!repoId) { Toast.info(__t('trash.select_library')); return; }
    var confirmed = await ConfirmDialog.confirm(
        __t('common.are_you_sure'),
        __t('trash.delete_all_confirm'),
        { confirmText: __t('common.delete'), variant: 'danger' }
    );
    if (!confirmed) return;
    mutate('/api/v2.1/repos/' + encodeURIComponent(repoId) + '/trash/', {
        method: 'DELETE',
    }).then(function (r) {
        if (!r) return;
        if (r.ok) window.location.reload();
        else Toast.error(__t('trash.clean_failed'));
    }).catch(function () {
        Toast.error(__t('common.network_error'));
    });
}


/**
 * Call the deleted-repos API, then reload the libraries tab with a status flag.
 *
 * The restore posts a form body, which is what seahub sends; the deletes carry
 * no body. Both need the CSRF header because the request rides the session
 * cookie.
 */
async function deletedReposRequest(method, path, body, flag) {
    var headers = {};
    if (body) headers['Content-Type'] = 'application/x-www-form-urlencoded';
    try {
        var resp = await mutate(path, { method: method, body: body, headers: headers });
        if (!resp) return;
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        window.location.href = '/trash/?tab=libraries&' + flag + '=true';
    } catch (err) {
        Toast.error(__t('trash.lib_failed'));
    }
}

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

async function deleteAllLibraries() {
    var rows = document.querySelectorAll('#tab-libraries .nf-prow').length;
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
