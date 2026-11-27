// repos — library list interactions (create/edit/delete, filter, sort and the row menu), driven
// by data-* event delegation.
import { __t } from "../core/i18n.js";
import { apiFetch, mutate } from "../core/api.js";
import { copyText, flashCopied } from "../core/clipboard.js";
import { registerModalClose } from "../core/modal.js";
import { Toast } from "../core/toast.js";
import { ConfirmDialog } from "../core/confirm.js";
import { registerRowMenu, addMenuItem, addMenuSeparator } from "../core/row-menu.js";
import { applySortIndicators, nextSortOrder } from "../browser/view-logic.js";

var reposPageEl = document.getElementById("repos-page");
var webdavBaseUrl = reposPageEl ? (reposPageEl.dataset.webdavBaseUrl || "") : "";
var isAdmin = reposPageEl ? reposPageEl.dataset.isAdmin === "1" : false;

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
    mutate('/api2/repos/', {
        headers: { 'X-CSRFToken': csrfToken },
        body: { name: name },
    }).then(function (resp) {
        if (!resp) return;
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
    copyText(document.getElementById('webdav-url').textContent).then(function (copied) {
        if (copied) flashCopied(document.getElementById('copy-url-btn'), __t('webdav.copied'));
    });
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
    mutate('/api2/repos/' + repoId + '/?op=update', {
        headers: { 'X-CSRFToken': csrfToken },
        body: {
            repo_name: newName,
            description: newDesc,
            history_limit: historyLimit,
            history_ttl_days: historyTtlDays,
        },
    }).then(function (resp) {
        if (!resp) return;
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
    var confirmed = await ConfirmDialog.confirm(
        __t('common.are_you_sure'),
        __t('repo.delete_confirm', { name: repoName }),
        { confirmText: __t('common.delete'), variant: 'danger' }
    );
    if (!confirmed) return;

    mutate('/api2/repos/' + repoId + '/', { method: 'DELETE' }).then(function (resp) {
        if (!resp) return;
        if (resp.ok) {
            window.location.reload();
        } else {
            Toast.error(__t('repo.delete_failed') + resp.status);
        }
    }).catch(function () {
        Toast.error(__t('common.network_error'));
    });
}

function numOrNull(id) {
    var v = document.getElementById(id).value.trim();
    if (v === '' || isNaN(Number(v))) return null;
    return Number(v);
}

function copyRepoWebdav(btn) {
    var url = webdavBaseUrl + '/dav/' + btn.getAttribute('data-repo-id') + '/';
    copyText(url).then(function (copied) {
        if (copied) Toast.success(__t('webdav.copied'));
    });
}

// Every library is already in the DOM (a user's list is bounded), so filter and sort are pure
// DOM work: no request, no pagination state, and the page still works without JavaScript.
var repoListEl = document.getElementById("repo-list");
var filterEl = document.getElementById("repo-filter");
var countEl = document.getElementById("repo-count");
var noMatchEl = document.getElementById("repo-no-match");
var sortGroup = document.querySelector(".js-repo-sort");
// The server renders name-ascending, the state the buttons and this module start from.
var listState = { key: "name", order: "asc", query: "" };

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

// Name A→Z, largest and newest. Name compares lowercased code points — the order ui/repos.rs
// renders, so the default paint is not re-sorted — and the direction is the file list's toggle.
function compareRepos(a, b) {
    var x, y;
    if (listState.key === "name") {
        x = (a.dataset.name || "").toLowerCase();
        y = (b.dataset.name || "").toLowerCase();
        var byName = x < y ? -1 : x > y ? 1 : 0;
        return listState.order === "asc" ? byName : -byName;
    }
    if (listState.key === "size") {
        x = Number(a.dataset.sizeBytes || 0);
        y = Number(b.dataset.sizeBytes || 0);
    } else {
        x = Number(a.dataset.mtime || 0);
        y = Number(b.dataset.mtime || 0);
    }
    return listState.order === "asc" ? x - y : y - x;
}

// Like the file list's sort UI: the active field carries `on` and the current direction is lit.
function applySortUI() {
    applySortIndicators(sortGroup, listState.key, listState.order, {
        btnSelector: ".js-repo-sort-btn",
    });
}

function applySort() {
    if (repoListEl) {
        var rows = repoRows().sort(compareRepos);
        // The server renders the default order, so rows are usually in place; re-appending would flicker.
        var moved = rows.some(function (row, i) { return repoListEl.children[i] !== row; });
        if (moved) {
            rows.forEach(function (row) { repoListEl.appendChild(row); });
        }
    }
    applySortUI();
}

// ── Sharing (administrators only) ──────────────────────────────────────────
// The list endpoint is the official client's `dir/shared_items/` shape: a bare
// array of user shares. Membership changes write through the same endpoints the
// desktop client uses, so both surfaces share one policy.
var shareRepoId = "";
var shareVisibility = "private";

function shareUrl(path) {
    return "/api2/repos/" + encodeURIComponent(shareRepoId) + "/dir/shared_items/?p=/" + path;
}

function showShareDialog(btn) {
    shareRepoId = btn.getAttribute("data-id");
    shareVisibility = btn.getAttribute("data-visibility") || "private";
    document.getElementById("share-repo-id").value = shareRepoId;
    document.getElementById("share-repo-name").textContent = btn.getAttribute("data-name") || "";
    Array.prototype.forEach.call(
        document.querySelectorAll('input[name="visibility"]'),
        function (radio) {
            radio.checked = radio.value === shareVisibility;
        }
    );
    updateShareMode();
    loadShareMembers();
    document.getElementById("share-overlay").classList.remove("hidden");
}

function hideShareDialog() {
    document.getElementById("share-overlay").classList.add("hidden");
}

function selectedVisibility() {
    var checked = document.querySelector('input[name="visibility"]:checked');
    return checked ? checked.value : "private";
}

function updateShareMode() {
    var members = document.getElementById("share-members");
    if (!members) return;
    // The member editor only applies to "selected users"; a public or private
    // library has nobody in the list.
    members.hidden = selectedVisibility() !== "shared";
}

function renderShareMembers(items) {
    var list = document.getElementById("share-member-list");
    if (!list) return;
    list.textContent = "";
    if (!items.length) {
        var empty = document.createElement("p");
        empty.className = "text-[12px] text-ink-2";
        empty.textContent = __t("repo.share_empty");
        list.appendChild(empty);
        return;
    }
    items.forEach(function (item) {
        var info = item.user_info || {};
        var email = info.name || "";
        var row = document.createElement("div");
        row.className = "flex items-center gap-2";
        var label = document.createElement("span");
        label.className = "flex-1 min-w-0 truncate text-[13px]";
        label.textContent = (info.nickname ? info.nickname + " " : "") + email;
        var select = document.createElement("select");
        select.className = "input w-[112px]";
        select.setAttribute("data-share-perm", email);
        [["r", __t("repo.permission_read")], ["rw", __t("repo.permission_write")]].forEach(function (opt) {
            var o = document.createElement("option");
            o.value = opt[0];
            o.textContent = opt[1];
            if (opt[0] === item.permission) o.selected = true;
            select.appendChild(o);
        });
        var remove = document.createElement("button");
        remove.type = "button";
        remove.className = "btn btn-line";
        remove.setAttribute("data-action", "share-remove");
        remove.setAttribute("data-email", email);
        remove.textContent = __t("repo.share_remove");
        row.appendChild(label);
        row.appendChild(select);
        row.appendChild(remove);
        list.appendChild(row);
    });
}

function loadShareMembers() {
    if (!shareRepoId) return;
    apiFetch(shareUrl("")).then(function (res) {
        return res.json();
    }).then(function (items) {
        items = Array.isArray(items) ? items : [];
        // A private row whose members appeared behind its back (the first add
        // switches the library to `shared`) must show the member editor instead
        // of silently hiding it.
        if (items.length && selectedVisibility() === "private") {
            var shared = document.querySelector('input[name="visibility"][value="shared"]');
            if (shared) shared.checked = true;
            updateShareMode();
        }
        renderShareMembers(items);
    }).catch(function () {
        renderShareMembers([]);
    });
}

function addShareMember() {
    var user = document.getElementById("share-user-select");
    var role = document.getElementById("share-permission-select");
    if (!user || !user.value) return;
    mutate(shareUrl(""), {
        method: "PUT",
        body: {
            share_type: "user",
            username: user.value,
            permission: role ? role.value : "r",
        },
    }).then(function (resp) {
        if (resp && resp.ok) {
            shareVisibility = "shared";
            var shared = document.querySelector('input[name="visibility"][value="shared"]');
            if (shared) shared.checked = true;
            updateShareMode();
            loadShareMembers();
        } else if (resp) Toast.error(__t("repo.share_failed") + resp.status);
    }).catch(function (error) {
        Toast.error(__t("repo.share_failed") + error.message);
    });
}

function updateShareMember(email, permission) {
    mutate(shareUrl("&share_type=user&username=" + encodeURIComponent(email)), {
        method: "POST",
        body: { permission: permission },
    }).then(function (resp) {
        if (resp && resp.ok) loadShareMembers();
        else if (resp) Toast.error(__t("repo.share_failed") + resp.status);
    }).catch(function (error) {
        Toast.error(__t("repo.share_failed") + error.message);
    });
}

function removeShareMember(email) {
    mutate(shareUrl("&share_type=user&username=" + encodeURIComponent(email)), {
        method: "DELETE",
    }).then(function (resp) {
        if (resp && resp.ok) loadShareMembers();
        else if (resp) Toast.error(__t("repo.share_failed") + resp.status);
    }).catch(function (error) {
        Toast.error(__t("repo.share_failed") + error.message);
    });
}

function saveShareVisibility(form) {
    var visibility = selectedVisibility();
    if (visibility === "public" && shareVisibility === "shared") {
        ConfirmDialog.confirm(
            __t("common.are_you_sure"),
            __t("repo.share_public_confirm"),
            { confirmText: __t("common.save") }
        ).then(function (ok) {
            if (ok) putShareVisibility(visibility);
        });
        return false;
    }
    putShareVisibility(visibility);
    return false;
}

function putShareVisibility(visibility) {
    mutate("/api/v2.1/repos/" + encodeURIComponent(shareRepoId) + "/visibility/", {
        method: "PUT",
        body: { visibility: visibility },
    }).then(function (resp) {
        if (resp && resp.ok) {
            hideShareDialog();
            window.location.reload();
        } else if (resp) {
            Toast.error(__t("repo.share_failed") + resp.status);
        }
    }).catch(function (error) {
        Toast.error(__t("repo.share_failed") + error.message);
    });
}

// The menu is built here, so the row carries only data: metadata columns and no buttons until
// it is hovered or focused.
function buildRepoMenu(row) {
    var isOwner = row.dataset.isOwner === "1";
    var menu = document.createElement("div");
    menu.className = "nf-menu";
    menu.setAttribute("role", "menu");

    addMenuItem(menu, {
        label: __t("repo.open"),
        href: "/libraries/" + encodeURIComponent(row.dataset.id) + "/files/",
    });
    if (isAdmin && isOwner) {
        addMenuItem(menu, {
            label: __t("repo.share_settings"),
            attrs: {
                "data-action": "show-share",
                "data-id": row.dataset.id,
                "data-name": row.dataset.name,
                "data-visibility": row.dataset.visibility,
            },
        });
    }
    if (isOwner) {
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
    }
    addMenuItem(menu, {
        label: __t("repo.copy_webdav"),
        attrs: { "data-action": "copy-repo-webdav", "data-repo-id": row.dataset.id },
    });
    if (isOwner) {
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
    }
    return menu;
}

registerRowMenu(function (btn) {
    var row = btn.closest("[data-lib]");
    return row ? buildRepoMenu(row) : null;
});

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

if (sortGroup) {
    sortGroup.addEventListener("click", function (e) {
        var btn = e.target.closest(".js-repo-sort-btn");
        if (!btn) return;
        // The same field flips direction; a new field starts ascending, like the file list.
        listState.order = nextSortOrder(btn.dataset.sort, listState.key, listState.order);
        listState.key = btn.dataset.sort;
        applySort();
    });
}

if (repoListEl) {
    applySort();
    applyFilter();
}

document.addEventListener("click", function (e) {
    var el = e.target.closest("[data-action]");
    if (!el) return;
    var action = el.dataset.action;
    switch (action) {
        case "show-create": showCreateDialog(); break;
        case "show-edit": showEditDialog(el); break;
        case "show-share": showShareDialog(el); break;
        case "delete-repo": deleteRepo(el); break;
        case "copy-webdav-url": copyWebdavUrl(); break;
        case "copy-repo-webdav": copyRepoWebdav(el); break;
        case "close-create": hideCreateDialog(); break;
        case "close-edit": hideEditDialog(); break;
        case "close-share": hideShareDialog(); break;
        case "share-add": addShareMember(); break;
        case "share-remove": removeShareMember(el.dataset.email); break;
    }
});

// Permission edits happen in a select, so a click handler cannot see them.
document.addEventListener("change", function (e) {
    var select = e.target.closest("[data-share-perm]");
    if (!select) return;
    updateShareMember(select.getAttribute("data-share-perm"), select.value);
});

document.addEventListener("submit", function (e) {
    var createForm = e.target.closest('[data-form="create"]');
    if (createForm) { e.preventDefault(); submitCreate(createForm); return; }
    var editForm = e.target.closest('[data-form="edit"]');
    if (editForm) { e.preventDefault(); submitEdit(editForm); return; }
    var shareForm = e.target.closest('[data-form="share"]');
    if (shareForm) { e.preventDefault(); saveShareVisibility(shareForm); return; }
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
registerModalClose("hideShareDialog", hideShareDialog);

Array.prototype.forEach.call(
    document.querySelectorAll('input[name="visibility"]'),
    function (radio) {
        radio.addEventListener("change", updateShareMode);
    }
);
