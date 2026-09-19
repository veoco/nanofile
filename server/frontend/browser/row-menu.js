// row-menu — the always-visible "⋯" on each file row opens a single shared
// menu mounted on <body>.
//
// The menu deliberately lives outside the list: it is rendered on <body> rather
// than inside the row, because a dropdown inside `.nf-scroll` would be clipped
// by the scroll container. Being detached means the menu cannot rely on
// `closest(".js-entry-row")` — so every item carries the row's `data-*` values
// itself, which is also what the existing delegated handlers in operations.js /
// upload-link-dialog.js read.
import { __t } from "../core/i18n.js";

var menuEl = null;
var openBtn = null;

// ─── Build ──────────────────────────────────────────────────────────────
function addItem(menu, opts) {
  var el = document.createElement(opts.href ? "a" : "button");
  el.className = "nf-menu-item" + (opts.danger ? " danger" : "");
  el.setAttribute("role", "menuitem");
  el.textContent = opts.label;
  if (el.tagName === "BUTTON") el.type = "button";
  if (opts.href) el.href = opts.href;
  if (opts.cls) el.className += " " + opts.cls;
  Object.keys(opts.attrs || {}).forEach(function (k) {
    var v = opts.attrs[k];
    if (v !== undefined && v !== null && v !== "") el.setAttribute(k, v);
  });
  menu.appendChild(el);
}

function addSeparator(menu) {
  var sep = document.createElement("div");
  sep.className = "nf-menu-sep";
  sep.setAttribute("role", "separator");
  menu.appendChild(sep);
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

  addItem(menu, { label: __t("fb.get_share_link"), cls: "js-share-btn", attrs: attrs });
  addItem(menu, {
    label: __t("fb.get_upload_link"),
    attrs: { "data-action": "open-upload-link", "data-repo-id": row.dataset.repoId, "data-path": row.dataset.path },
  });

  addSeparator(menu);

  if (type === "dir") {
    // Directories are downloaded as a zip through the delegated handler.
    addItem(menu, { label: __t("common.download"), cls: "js-entry-download", attrs: attrs });
  } else {
    // A plain link: adding `.js-entry-download` here would route the file
    // through the zip handler instead of streaming it directly.
    addItem(menu, {
      label: __t("common.download"),
      href: "/repos/" + encodeURIComponent(row.dataset.repoId) + "/files/" + row.dataset.path + "?dl=1",
      attrs: attrs,
    });
    addItem(menu, { label: __t("fb.history"), cls: "js-history-btn", attrs: attrs });
  }

  addItem(menu, { label: __t("common.rename"), cls: "js-rename-btn", attrs: attrs });
  addItem(menu, { label: __t("common.delete"), cls: "js-delete-btn", attrs: attrs, danger: true });

  return menu;
}

// ─── Placement ──────────────────────────────────────────────────────────
function position(menu, btn) {
  var r = btn.getBoundingClientRect();
  var mw = menu.offsetWidth;
  var mh = menu.offsetHeight;
  var left = Math.min(r.right - mw, window.innerWidth - mw - 8);
  left = Math.max(8, left);
  var top = r.bottom + 4;
  if (top + mh > window.innerHeight - 8) {
    top = Math.max(8, r.top - mh - 4);
  }
  menu.style.left = left + "px";
  menu.style.top = top + "px";
}

// ─── Open / close ───────────────────────────────────────────────────────
export function closeRowMenu(restoreFocus) {
  if (menuEl) {
    menuEl.remove();
    menuEl = null;
  }
  if (openBtn) {
    openBtn.setAttribute("aria-expanded", "false");
    if (restoreFocus) openBtn.focus();
    openBtn = null;
  }
}

function openRowMenu(btn) {
  var row = btn.closest(".js-entry-row");
  if (!row) return;
  closeRowMenu(false);

  var menu = buildMenu(row);
  // Measure off-screen, then place.
  menu.style.visibility = "hidden";
  document.body.appendChild(menu);
  position(menu, btn);
  menu.style.visibility = "";

  menuEl = menu;
  openBtn = btn;
  btn.setAttribute("aria-expanded", "true");

  var first = menu.querySelector(".nf-menu-item");
  if (first) first.focus();
}

// ─── Wiring ─────────────────────────────────────────────────────────────
document.addEventListener("click", function (e) {
  var btn = e.target.closest(".nf-more");
  if (btn) {
    e.stopPropagation();
    if (menuEl && openBtn === btn) {
      closeRowMenu(false);
    } else {
      openRowMenu(btn);
    }
    return;
  }
  // An item was activated (or anything else was clicked) — dismiss.
  if (menuEl && !e.target.closest(".nf-menu")) closeRowMenu(false);
  // The menu's own actions open modals; give the modal a clean slate.
  if (menuEl && e.target.closest(".nf-menu-item")) closeRowMenu(false);
});

document.addEventListener("keydown", function (e) {
  if (!menuEl) return;
  if (e.key === "Escape") {
    e.preventDefault();
    closeRowMenu(true);
    return;
  }
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    var items = Array.prototype.slice.call(menuEl.querySelectorAll(".nf-menu-item"));
    var idx = items.indexOf(document.activeElement);
    var next = e.key === "ArrowDown" ? idx + 1 : idx - 1;
    if (next < 0) next = items.length - 1;
    if (next >= items.length) next = 0;
    items[next].focus();
  }
});

// The menu is anchored to a viewport rect, so anything that moves the row —
// scrolling (capture: scroll events do not bubble), resizing, or leaving the
// window — invalidates it.
window.addEventListener("scroll", function () { closeRowMenu(false); }, true);
window.addEventListener("resize", function () { closeRowMenu(false); });
window.addEventListener("blur", function () { closeRowMenu(false); });
