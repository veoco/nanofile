// shares — tab switching (shared by /shares/ and /sysadmin/shares/) plus the
// edit-dialog logic for the user's own shares page.
// Migrated from the inline <script> blocks in templates/shares/list.html and
// templates/adminshares/list.html (which duplicated switchTab).
import { __t } from "../core/i18n.js";
import { apiFetch } from "../core/api.js";
import { switchTab } from "../core/tabs.js";

// ─── Tab switching ─────────────────────────────────────────────────────
function selectTab(name) {
    switchTab(name, 'share-links');

    // Keep delete form hidden inputs in sync, so a delete reloads the tab the
    // user was looking at.
    document.querySelectorAll('.delete-form input[name="tab"]').forEach(function (input) {
        input.value = name === 'upload-links' ? 'upload-links' : '';
    });
}

document.addEventListener("click", function (e) {
    var el = e.target.closest('[data-action="tab"]');
    if (!el) return;
    selectTab(el.dataset.tab);
});

// ─── Edit share link dialog ──────────────────────────────────────────
(function () {
    var overlay = document.getElementById("share-edit-overlay");
    if (!overlay) return;
    var tokenInput = document.getElementById("edit-token-input");
    var expirySelect = document.getElementById("edit-expiry-select");
    var passwordInput = document.getElementById("edit-password-input");
    var descInput = document.getElementById("edit-description-input");
    var pathDisplay = document.querySelector(".js-edit-share-path");
    var errorDiv = document.querySelector(".js-edit-share-error");
    var confirmBtn = document.querySelector(".js-edit-confirm");
    var editToken = "";

    document.addEventListener("click", function (e) {
        var btn = e.target.closest(".js-share-edit-btn");
        if (!btn) return;
        editToken = btn.dataset.token;
        pathDisplay.textContent = btn.dataset.path;
        tokenInput.value = editToken;
        expirySelect.value = "";
        passwordInput.value = "";
        document.getElementById("edit-password-clear").checked = false;
        descInput.value = btn.dataset.description || "";
        errorDiv.classList.add("hidden");
        confirmBtn.disabled = false;
        confirmBtn.textContent = __t('common.save');
        overlay.classList.remove("hidden");
    });

    overlay.querySelector(".js-edit-cancel").addEventListener("click", function () {
        overlay.classList.add("hidden");
    });
    overlay.addEventListener("click", function (e) {
        if (e.target === overlay) overlay.classList.add("hidden");
    });

    confirmBtn.addEventListener("click", async function () {
        var body = {};

        var clearPwd = document.getElementById("edit-password-clear").checked;
        var pwd = passwordInput.value.trim();
        if (clearPwd) {
            body.password = null;
        } else if (pwd) {
            body.password = pwd;
        }

        var expiryVal = expirySelect.value;
        if (expiryVal === "clear") {
            body.expire_days = null;
        } else if (expiryVal) {
            body.expire_days = parseInt(expiryVal, 10);
        }

        var desc = descInput.value.trim();
        body.description = desc || null;

        confirmBtn.disabled = true;
        confirmBtn.textContent = __t('common.saving');
        errorDiv.classList.add("hidden");

        try {
            var resp = await apiFetch("/api/v2.1/share-links/" + editToken + "/", {
                method: "PUT",
                body: JSON.stringify(body),
            });
            if (resp.ok) {
                overlay.classList.add("hidden");
                if (window.refreshFileList) window.refreshFileList();
                else window.location.reload();
            } else {
                var text = await resp.text().catch(function () { return resp.statusText; });
                errorDiv.textContent = text;
                errorDiv.classList.remove("hidden");
            }
        } catch (err) {
            errorDiv.textContent = err.message;
            errorDiv.classList.remove("hidden");
        } finally {
            confirmBtn.disabled = false;
            confirmBtn.textContent = __t('common.save');
        }
    });
})();

// ─── Edit upload link dialog ──────────────────────────────────────────
(function () {
    var overlay = document.getElementById("ul-edit-overlay");
    if (!overlay) return;
    var tokenInput = document.getElementById("ul-edit-token-input");
    var expirySelect = document.getElementById("ul-edit-expiry-select");
    var passwordInput = document.getElementById("ul-edit-password-input");
    var descInput = document.getElementById("ul-edit-description-input");
    var pathDisplay = document.querySelector(".js-ul-edit-path");
    var errorDiv = document.querySelector(".js-ul-edit-error");
    var confirmBtn = document.querySelector(".js-ul-edit-confirm");
    var editToken = "";

    document.addEventListener("click", function (e) {
        var btn = e.target.closest(".js-ul-edit-btn");
        if (!btn) return;
        editToken = btn.dataset.token;
        pathDisplay.textContent = btn.dataset.path;
        tokenInput.value = editToken;
        expirySelect.value = "";
        passwordInput.value = "";
        document.getElementById("ul-edit-password-clear").checked = false;
        descInput.value = btn.dataset.description || "";
        errorDiv.classList.add("hidden");
        confirmBtn.disabled = false;
        confirmBtn.textContent = __t('common.save');
        overlay.classList.remove("hidden");
    });

    overlay.querySelector(".js-ul-edit-cancel").addEventListener("click", function () {
        overlay.classList.add("hidden");
    });
    overlay.addEventListener("click", function (e) {
        if (e.target === overlay) overlay.classList.add("hidden");
    });

    confirmBtn.addEventListener("click", async function () {
        var body = {};
        var clearPwd = document.getElementById("ul-edit-password-clear").checked;
        var pwd = passwordInput.value.trim();
        if (clearPwd) {
            body.password = null;
        } else if (pwd) {
            body.password = pwd;
        }
        var expiryVal = expirySelect.value;
        if (expiryVal === "clear") {
            body.expire_days = null;
        } else if (expiryVal) {
            body.expire_days = parseInt(expiryVal, 10);
        }
        var desc = descInput.value.trim();
        body.description = desc || null;

        confirmBtn.disabled = true;
        confirmBtn.textContent = __t('common.saving');
        errorDiv.classList.add("hidden");

        try {
            var resp = await apiFetch("/api/v2.1/upload-links/" + editToken + "/", {
                method: "PUT",
                body: JSON.stringify(body),
            });
            if (resp.ok) {
                overlay.classList.add("hidden");
                window.location.reload();
            } else {
                var text = await resp.text().catch(function () { return resp.statusText; });
                errorDiv.textContent = text;
                errorDiv.classList.remove("hidden");
            }
        } catch (err) {
            errorDiv.textContent = err.message;
            errorDiv.classList.remove("hidden");
        } finally {
            confirmBtn.disabled = false;
            confirmBtn.textContent = __t('common.save');
        }
    });
})();
