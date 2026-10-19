// file-browser row menu — the items behind the "⋯" on a file or folder row.
// The dropdown itself (placement, dismissal, keyboard) lives in core/row-menu.
import { __t } from "../core/i18n.js";
import { registerRowMenu, addMenuItem, addMenuSeparator } from "../core/row-menu.js";

// The server puts the two sharing switches on `#nf-content`; a menu item for a
// closed direction would open a dialog the server did not render. A page without
// the element (nothing else loads this module today) keeps both.
var content = document.getElementById("nf-content");

function directionEnabled(flag) {
  return !content || content.dataset[flag] !== "false";
}

function buildMenu(row) {
  var type = row.dataset.type || "file";
  var attrs = {
    "data-repo-id": row.dataset.repoId,
    "data-path": row.dataset.path,
    "data-name": row.dataset.name,
    "data-type": type,
  };
  var menu = document.createElement("div");
  menu.className = "nf-menu";
  menu.setAttribute("role", "menu");

  var canShareDownload = directionEnabled("shareDownloadEnabled");
  var canShareUpload = directionEnabled("shareUploadEnabled");
  if (canShareDownload || canShareUpload) {
    if (canShareDownload) {
      addMenuItem(menu, { label: __t("fb.get_share_link"), cls: "js-share-btn", attrs: attrs });
    }
    if (canShareUpload) {
      addMenuItem(menu, {
        label: __t("fb.get_upload_link"),
        attrs: { "data-action": "open-upload-link", "data-repo-id": row.dataset.repoId, "data-path": row.dataset.path },
      });
    }
    addMenuSeparator(menu);
  }

  if (type === "dir") {
    // Directories are downloaded as a zip through the delegated handler.
    addMenuItem(menu, { label: __t("common.download"), cls: "js-entry-download", attrs: attrs });
  } else {
    // A plain link: adding `.js-entry-download` here would route the file
    // through the zip handler instead of streaming it directly.
    addMenuItem(menu, {
      label: __t("common.download"),
      href: "/repos/" + encodeURIComponent(row.dataset.repoId) + "/files/" + row.dataset.path + "?dl=1",
      attrs: attrs,
    });
    addMenuItem(menu, { label: __t("fb.history"), cls: "js-history-btn", attrs: attrs });
  }

  addMenuItem(menu, { label: __t("common.rename"), cls: "js-rename-btn", attrs: attrs });
  addMenuItem(menu, { label: __t("common.delete"), cls: "js-delete-btn", attrs: attrs, danger: true });

  return menu;
}

registerRowMenu(function (btn) {
  var row = btn.closest(".js-entry-row");
  return row ? buildMenu(row) : null;
});
