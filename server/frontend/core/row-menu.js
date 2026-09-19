// row-menu — one shared dropdown for the "⋯" button of any row list.
//
// The menu is built by whichever feature owns the list (see `registerRowMenu`)
// and mounted on <body>: a dropdown inside a scrolling container would be
// clipped by it. Being detached means the builder cannot rely on
// `closest(".js-entry-row")` from inside an item — it receives the clicked
// button and returns the menu it wants, with every item already carrying the
// `data-*` values its delegated handler reads.
//
// The state lives on `window` because a page can load two bundles (common and
// file-browser) that both contain this module: without the guard each copy
// would install its own document listeners and one click would open two menus.

var KEY = "__nfRowMenu";

function state() {
  if (window[KEY]) return window[KEY];
  var s = { builders: [], menu: null, btn: null };
  window[KEY] = s;
  install(s);
  return s;
}

// ─── Item helpers (shared by every builder) ─────────────────────────────
export function addMenuItem(menu, opts) {
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

export function addMenuSeparator(menu) {
  var sep = document.createElement("div");
  sep.className = "nf-menu-sep";
  sep.setAttribute("role", "separator");
  menu.appendChild(sep);
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
function close(restoreFocus) {
  var s = state();
  if (s.menu) {
    s.menu.remove();
    s.menu = null;
  }
  if (s.btn) {
    s.btn.setAttribute("aria-expanded", "false");
    if (restoreFocus) s.btn.focus();
    s.btn = null;
  }
}

function open(btn) {
  var s = state();
  close(false);

  var menu = null;
  for (var i = 0; i < s.builders.length && !menu; i++) {
    menu = s.builders[i](btn);
  }
  if (!menu) return;

  // Measure off-screen, then place.
  menu.style.visibility = "hidden";
  document.body.appendChild(menu);
  position(menu, btn);
  menu.style.visibility = "";

  s.menu = menu;
  s.btn = btn;
  btn.setAttribute("aria-expanded", "true");

  var first = menu.querySelector(".nf-menu-item");
  if (first) first.focus();
}

// ─── Wiring ─────────────────────────────────────────────────────────────
function install(s) {
  document.addEventListener("click", function (e) {
    var btn = e.target.closest(".nf-more");
    if (btn) {
      e.stopPropagation();
      if (s.menu && s.btn === btn) {
        close(false);
      } else {
        open(btn);
      }
      return;
    }
    // An item was activated (or anything else was clicked) — dismiss. The
    // delegated handler that runs afterwards still sees the item's `data-*`
    // values, because a detached element keeps its attributes.
    if (s.menu && !e.target.closest(".nf-menu")) close(false);
    if (s.menu && e.target.closest(".nf-menu-item")) close(false);
  });

  document.addEventListener("keydown", function (e) {
    if (!s.menu) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close(true);
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      var items = Array.prototype.slice.call(s.menu.querySelectorAll(".nf-menu-item"));
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
  window.addEventListener("scroll", function () { close(false); }, true);
  window.addEventListener("resize", function () { close(false); });
  window.addEventListener("blur", function () { close(false); });
}

/// Register a builder for a row list. `build` receives the clicked `.nf-more`
/// button and returns a `.nf-menu` element, or null if the button belongs to a
/// different list (builders are tried in registration order).
export function registerRowMenu(build) {
  state().builders.push(build);
}
