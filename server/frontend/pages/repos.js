// repos — library list page interactions (create/edit/delete library, the
// list's own filter and sort, and the per-row "⋯" menu). Migrated from the
// inline <script> block that used to live in templates/repos/list.html; now
// driven by data-* event delegation.
import { __t } from "../core/i18n.js";
import { getCookie } from "../core/utils.js";
import { registerModalClose } from "../core/modal.js";
import { Toast } from "../core/toast.js";
import { ConfirmDialog } from "../core/confirm.js";
import { registerRowMenu, addMenuItem, addMenuSeparator } from "../core/row-menu.js";

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
            Toast.error(__t('repo.create_failed') + resp.status);
        }
    }).catch(function () {
        Toast.error(__t('common.network_error'));
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
    if (!newName) { Toast.info(__t('repo.name_empty')); return false; }
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
            Toast.error(__t('repo.update_failed') + resp.status);
        }
    }).catch(function () {
        Toast.error(__t('common.network_error'));
    });
    return false;
}

async function deleteRepo(btn) {
    var repoId = btn.getAttribute('data-repo-id');
    var repoName = btn.getAttribute('data-repo-name');
    var csrfToken = getCookie('sfcsrftoken');
    if (!csrfToken) { window.location.href = '/accounts/login/'; return; }

    var confirmed = await ConfirmDialog.confirm(
        __t('common.are_you_sure'),
        __t('repo.delete_confirm', { name: repoName }),
        { confirmText: __t('common.delete'), variant: 'danger' }
    );
    if (!confirmed) return;

    fetch('/api2/repos/' + repoId + '/', {
        method: 'DELETE',
        credentials: 'same-origin',
        headers: { 'X-CSRFToken': csrfToken },
    }).then(function (resp) {
        if (resp.ok) {
            window.location.reload();
        } else {
            Toast.error(__t('repo.delete_failed') + resp.status);
        }
    }).catch(function () {
        Toast.error(__t('common.network_error'));
    });
}

// Read a numeric input; empty/invalid → null so the server leaves it unchanged.
function numOrNull(id) {
    var v = document.getElementById(id).value.trim();
    if (v === '' || isNaN(Number(v))) return null;
    return Number(v);
}

function copyRepoWebdav(btn) {
    var url = webdavBaseUrl + '/dav/' + btn.getAttribute('data-repo-id') + '/';
    if (navigator.clipboard) {
        navigator.clipboard.writeText(url).catch(function () {});
    }
    Toast.success(__t('webdav.copied'));
}

// ─── The list: filter, sort, row menu ───────────────────────────────────
// Every library is already in the DOM (a user's own list is bounded), so both
// the filter and the sort are pure DOM work: no request, no pagination state,
// and the page still works with JavaScript off — just unsorted and unfiltered.
var repoListEl = document.getElementById("repo-list");
var filterEl = document.getElementById("repo-filter");
var countEl = document.getElementById("repo-count");
var noMatchEl = document.getElementById("repo-no-match");
var sortBtn = document.getElementById("repo-sort-btn");
var sortPop = document.getElementById("repo-sort-pop");
var sortLabel = document.getElementById("repo-sort-label");
var listState = { key: "mtime", query: "" };

function repoRows() {
    if (!repoListEl) return [];
    return Array.prototype.slice.call(repoListEl.querySelectorAll("[data-lib]"));
}

function updateCount(shown) {
    if (!countEl) return;
    countEl.textContent = __t(shown === 1 ? "repo.count_one" : "repo.count_many", { n: String(shown) });
}

function applyFilter() {
    var q = listState.query.trim().toLowerCase();
    var shown = 0;
    repoRows().forEach(function (row) {
        var hay = ((row.dataset.name || "") + " " + (row.dataset.description || "")).toLowerCase();
        var hit = !q || hay.indexOf(q) !== -1;
        row.hidden = !hit;
        if (hit) shown++;
    });
    updateCount(shown);
    if (noMatchEl) {
        noMatchEl.hidden = shown !== 0;
        noMatchEl.textContent = shown === 0 ? __t("repo.no_match", { q: listState.query.trim() }) : "";
    }
}

// Newest, largest and A→Z: the order a person scans a library list in. There is
// no ascending variant — the file list's arrows cover that need; here the
// question is only ever "what did I touch last / what is eating the disk".
function sortValue(row) {
    if (listState.key === "name") return (row.dataset.name || "").toLowerCase();
    if (listState.key === "size") return -Number(row.dataset.sizeBytes || 0);
    return -Number(row.dataset.mtime || 0);
}

function applySort() {
    if (repoListEl) {
        var rows = repoRows().sort(function (a, b) {
            var x = sortValue(a);
            var y = sortValue(b);
            return typeof x === "string" ? x.localeCompare(y) : x - y;
        });
        // The server renders the default order, so on first load the rows are
        // usually already in place; re-appending them would only flicker.
        var moved = rows.some(function (row, i) { return repoListEl.children[i] !== row; });
        if (moved) {
            rows.forEach(function (row) { repoListEl.appendChild(row); });
        }
    }
    if (!sortPop) return;
    sortPop.querySelectorAll(".nf-pop-item").forEach(function (item) {
        var on = item.dataset.sort === listState.key;
        item.setAttribute("aria-checked", String(on));
        if (on && sortLabel) sortLabel.textContent = item.textContent.trim();
    });
}

// The menu is built here rather than from a server-rendered row, so the row
// itself carries only data: five columns of metadata and no buttons until the
// row is hovered or focused.
function buildRepoMenu(row) {
    var menu = document.createElement("div");
    menu.className = "nf-menu";
    menu.setAttribute("role", "menu");

    addMenuItem(menu, {
        label: __t("repo.open"),
        href: "/libraries/" + encodeURIComponent(row.dataset.id) + "/files/",
    });
    addMenuItem(menu, {
        label: __t("repo.edit_library"),
        attrs: {
            "data-action": "show-edit",
            "data-id": row.dataset.id,
            "data-name": row.dataset.name,
            "data-description": row.dataset.description,
            "data-size": row.dataset.sizeDisplay,
            "data-history-limit": row.dataset.historyLimit,
            "data-history-ttl-days": row.dataset.historyTtlDays,
        },
    });
    addMenuItem(menu, {
        label: __t("repo.copy_webdav"),
        attrs: { "data-action": "copy-repo-webdav", "data-repo-id": row.dataset.id },
    });
    addMenuSeparator(menu);
    addMenuItem(menu, {
        label: __t("common.delete"),
        danger: true,
        attrs: {
            "data-action": "delete-repo",
            "data-repo-id": row.dataset.id,
            "data-repo-name": row.dataset.name,
        },
    });
    return menu;
}

registerRowMenu(function (btn) {
    var row = btn.closest("[data-lib]");
    return row ? buildRepoMenu(row) : null;
});

function closeSortPop() {
    if (!sortPop || sortPop.hidden) return;
    sortPop.hidden = true;
    if (sortBtn) sortBtn.setAttribute("aria-expanded", "false");
}

if (filterEl) {
    filterEl.addEventListener("input", function () {
        listState.query = filterEl.value;
        applyFilter();
    });
    // Chrome clears a `type=search` field on Escape, Firefox and Safari do not.
    filterEl.addEventListener("keydown", function (e) {
        if (e.key === "Escape" && filterEl.value !== "") {
            e.preventDefault();
            filterEl.value = "";
            listState.query = "";
            applyFilter();
        }
    });
}

if (sortBtn && sortPop) {
    sortBtn.addEventListener("click", function (e) {
        e.stopPropagation();
        sortPop.hidden = !sortPop.hidden;
        sortBtn.setAttribute("aria-expanded", String(!sortPop.hidden));
    });
    sortPop.addEventListener("click", function (e) {
        var item = e.target.closest(".nf-pop-item");
        if (!item) return;
        listState.key = item.dataset.sort;
        applySort();
        sortPop.hidden = true;
        sortBtn.setAttribute("aria-expanded", "false");
        sortBtn.focus();
    });
    document.addEventListener("click", function (e) {
        if (e.target.closest("#repo-sort-pop") || e.target.closest("#repo-sort-btn")) return;
        closeSortPop();
    });
    document.addEventListener("keydown", function (e) {
        if (e.key !== "Escape" || sortPop.hidden) return;
        e.preventDefault();
        closeSortPop();
        sortBtn.focus();
    });
}

if (repoListEl) {
    applySort();
    applyFilter();
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
        case "copy-repo-webdav": copyRepoWebdav(el); break;
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
