// API-key settings page.
//
// The page is server-rendered so it works without JavaScript; this module only
// adds two conveniences: copying the freshly minted secret and pre-ticking the
// capability checkboxes of a chosen preset.

(function () {
    "use strict";

    function copyNewKey() {
        var input = document.getElementById("new-key-value");
        if (!input) return;
        input.select();
        input.setSelectionRange(0, input.value.length);
        navigator.clipboard.writeText(input.value).catch(function () {});
        var btn = document.getElementById("copy-new-key-btn");
        if (!btn) return;
        var original = btn.textContent;
        btn.textContent = __t("apikey.copied");
        setTimeout(function () {
            btn.textContent = original;
        }, 1500);
    }

    // A preset only ticks boxes; the server still validates and expands the
    // implications, so a mistake here cannot widen what a key may do.
    function applyPreset(select) {
        var option = select.options[select.selectedIndex];
        var caps = (option && option.dataset.caps ? option.dataset.caps : "").trim();
        if (!caps) return;
        var wanted = caps.split(/\s+/);
        var form = document.getElementById("create-key-form");
        if (!form) return;
        form.querySelectorAll('input[name^="cap__"]').forEach(function (box) {
            var id = box.name.slice("cap__".length);
            box.checked = wanted.indexOf(id) !== -1;
        });
    }

    document.addEventListener("click", function (e) {
        var el = e.target.closest('[data-action="copy-api-key"]');
        if (el) copyNewKey();
    });

    document.addEventListener("change", function (e) {
        if (e.target && e.target.id === "apikey-preset") {
            applyPreset(e.target);
        }
    });
})();
