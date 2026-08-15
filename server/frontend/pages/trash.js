// trash — empty (clean) the trash for a selected library.
// Migrated from the inline <script> block in templates/trash/list.html.
import { __t } from "../core/i18n.js";
import { getCookie } from "../core/utils.js";
import { registerModalClose } from "../core/modal.js";

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

document.addEventListener("click", function (e) {
    var el = e.target.closest("[data-action]");
    if (!el) return;
    if (el.dataset.action === "close-clean") hideCleanDialog();
    else if (el.dataset.action === "clean-trash") cleanTrash();
});

registerModalClose("hideCleanDialog", hideCleanDialog);
