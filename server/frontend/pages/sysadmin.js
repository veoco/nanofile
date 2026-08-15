// sysadmin — user management page interactions (create/edit/delete user dialogs).
// Migrated from the inline <script> block in templates/sysadmin/index.html.
import { registerModalClose } from "../core/modal.js";

function openCreate() {
    document.getElementById('create-overlay').classList.remove('hidden');
}
function closeCreate() {
    document.getElementById('create-overlay').classList.add('hidden');
}

function openEdit(el) {
    document.querySelector('.js-edit-email').textContent = el.dataset.email;
    document.getElementById('edit-is-admin').checked = el.dataset.isAdmin === 'true';
    document.getElementById('edit-is-active').checked = el.dataset.isActive === 'true';
    document.getElementById('edit-quota').value = el.dataset.quota !== '' ? el.dataset.quota : '';
    document.getElementById('edit-form').action = '/sysadmin/users/' + el.dataset.id + '/update/';
    document.getElementById('edit-overlay').classList.remove('hidden');
}

function closeEdit() {
    document.getElementById('edit-overlay').classList.add('hidden');
}

function openDelete(el) {
    document.getElementById('delete-email').textContent = el.dataset.email;
    document.getElementById('delete-form').action = '/sysadmin/users/' + el.dataset.id + '/delete/';
    document.getElementById('delete-overlay').classList.remove('hidden');
}

function closeDelete() {
    document.getElementById('delete-overlay').classList.add('hidden');
}

document.addEventListener("click", function (e) {
    var el = e.target.closest("[data-action]");
    if (!el) return;
    switch (el.dataset.action) {
        case "open-create": openCreate(); break;
        case "open-edit": openEdit(el); break;
        case "open-delete": openDelete(el); break;
        case "close-create": closeCreate(); break;
        case "close-edit": closeEdit(); break;
        case "close-delete": closeDelete(); break;
    }
});

registerModalClose("closeCreate", closeCreate);
registerModalClose("closeEdit", closeEdit);
registerModalClose("closeDelete", closeDelete);
