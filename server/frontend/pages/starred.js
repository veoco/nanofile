// starred — unstar an item from the starred list (DELETE, then reload).
// Migrated from the inline <script> block in templates/starred/list.html.
import { __t } from "../core/i18n.js";
import { getCookie } from "../core/utils.js";

document.addEventListener("click", function (e) {
    var btn = e.target.closest('[data-action="unstar"]');
    if (!btn) return;
    var repoId = btn.dataset.repoId;
    var path = btn.dataset.path;
    if (!confirm(__t('starred.unstar_confirm'))) return;
    var csrfToken = getCookie('sfcsrftoken');
    if (!csrfToken) {
        window.location.href = '/accounts/login/';
        return;
    }
    fetch('/api/v2.1/starred-items/?repo_id=' + encodeURIComponent(repoId) + '&path=' + encodeURIComponent(path), {
        method: 'DELETE',
        credentials: 'same-origin',
        headers: { 'X-CSRFToken': csrfToken },
    }).then(function (r) {
        if (r.ok) window.location.reload();
        else alert(__t('starred.unstar_failed') + r.status);
    }).catch(function () {
        alert(__t('common.network_error'));
    });
});
