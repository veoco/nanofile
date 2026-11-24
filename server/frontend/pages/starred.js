// starred — unstar an item from the starred list (DELETE, then reload).
// Migrated from the inline <script> block in templates/starred/list.html.
import { __t } from "../core/i18n.js";
import { mutate } from "../core/api.js";
import { Toast } from "../core/toast.js";
import { ConfirmDialog } from "../core/confirm.js";

document.addEventListener("click", async function (e) {
    var btn = e.target.closest('[data-action="unstar"]');
    if (!btn) return;
    var repoId = btn.dataset.repoId;
    var path = btn.dataset.path;
    var confirmed = await ConfirmDialog.confirm(
        __t('common.are_you_sure'),
        __t('starred.unstar_confirm'),
        { confirmText: __t('common.confirm') }
    );
    if (!confirmed) return;
    mutate('/api/v2.1/starred-items/?repo_id=' + encodeURIComponent(repoId) + '&path=' + encodeURIComponent(path), {
        method: 'DELETE',
    }).then(function (r) {
        if (!r) return;
        if (r.ok) window.location.reload();
        else Toast.error(__t('starred.unstar_failed') + r.status);
    }).catch(function () {
        Toast.error(__t('common.network_error'));
    });
});
