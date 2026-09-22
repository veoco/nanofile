// nav — global chrome interactions: mobile panel, user menu, dark mode,
// quick search, keyboard shortcuts, and the star toggle. Loaded once for all
// pages (side effects only).
import { __t } from "./i18n.js";
import { getCookie } from "./utils.js";
import { formatFileSize } from "./format.js";

// ─── Mobile left panel toggle ──────────────────────────────────────────
const menuToggle = document.querySelector(".js-mobile-menu-toggle");
const leftPanel = document.querySelector(".js-left-panel");

function toggleMobilePanel() {
  if (!leftPanel) return;
  if (leftPanel.classList.contains("hidden")) {
    leftPanel.classList.remove("hidden");
    leftPanel.classList.add("flex");
    leftPanel.style.width = "var(--left-panel-width, 240px)";
  } else {
    leftPanel.classList.add("hidden");
    leftPanel.classList.remove("flex");
    leftPanel.style.width = "0";
  }
}

if (menuToggle) {
  menuToggle.addEventListener("click", function (e) {
    e.stopPropagation();
    toggleMobilePanel();
  });
}

// ─── User menu dropdown (Sign out + User Management for admins) ─────────
const userMenu = document.querySelector(".js-user-menu");
const userButton = document.querySelector(".js-user-menu-button");
if (userMenu && userButton) {
  userButton.addEventListener("click", function (e) {
    e.stopPropagation();
    let dropdown = userMenu.querySelector(".js-user-menu-dropdown");
    if (dropdown) { dropdown.remove(); return; }
    dropdown = document.createElement("div");
    dropdown.className =
      "js-user-menu-dropdown absolute right-0 z-dialog mt-2 w-44 origin-top-right rounded-box border border-line bg-panel py-1 focus:outline-none";

    // Admin-only: User Management link
    var isAdmin = userMenu.getAttribute("data-is-admin") === "true";
    var menuItem = "block px-4 h-8 leading-8 text-[13px] text-ink hover:bg-raised";
    if (isAdmin) {
      var adminLink = document.createElement("a");
      adminLink.href = "/sysadmin/users/";
      adminLink.className = menuItem;
      adminLink.textContent = __t('ui.user_management');
      dropdown.appendChild(adminLink);

      var shareLink = document.createElement("a");
      shareLink.href = "/sysadmin/shares/";
      shareLink.className = menuItem;
      shareLink.textContent = __t('ui.share_management');
      dropdown.appendChild(shareLink);

      var taskLink = document.createElement("a");
      taskLink.href = "/sysadmin/tasks/";
      taskLink.className = menuItem;
      taskLink.textContent = __t('ui.task_management');
      dropdown.appendChild(taskLink);

      var emailLink = document.createElement("a");
      emailLink.href = "/sysadmin/email/";
      emailLink.className = menuItem;
      emailLink.textContent = __t('ui.email_management');
      dropdown.appendChild(emailLink);

      // System management: the settings pages, where every runtime-editable
      // setting lives (including the email configuration the page above no
      // longer owns).
      var settingsLink = document.createElement("a");
      settingsLink.href = "/sysadmin/settings/";
      settingsLink.className = menuItem;
      settingsLink.textContent = __t('ui.system_settings');
      dropdown.appendChild(settingsLink);
    }

    var signOut = document.createElement("a");
    signOut.href = "/accounts/logout/";
    signOut.className = menuItem;
    signOut.textContent = __t('ui.sign_out');
    dropdown.appendChild(signOut);

    userMenu.appendChild(dropdown);

    document.addEventListener(
      "click",
      function closeMenu(ev) {
        if (!userMenu.contains(ev.target)) {
          dropdown.remove();
          document.removeEventListener("click", closeMenu);
        }
      },
      { once: true }
    );
  });
}

// ─── Dark mode toggle ──────────────────────────────────────────────────
const darkToggle = document.querySelector(".js-dark-toggle");
if (darkToggle) {
  darkToggle.addEventListener("click", function () {
    document.documentElement.classList.toggle("dark");
    localStorage.setItem(
      "darkMode",
      document.documentElement.classList.contains("dark")
    );
  });
  if (localStorage.getItem("darkMode") === "true") {
    document.documentElement.classList.add("dark");
  }
}

// ─── Quick search ──────────────────────────────────────────────────────
var searchInput = document.querySelector(".js-quick-search");
if (searchInput) {
  searchInput.addEventListener("keydown", function (e) {
    if (e.key === "Enter") {
      var q = searchInput.value.trim();
      if (q) window.location.href = "/search/?q=" + encodeURIComponent(q);
    }
  });
}

// ─── Keyboard shortcuts ────────────────────────────────────────────────
var searchFocused = false;
document.addEventListener("keydown", function (e) {
  var tag = (e.target && e.target.tagName) || "";
  var isInput = tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";

  // "/" key to focus search
  if (e.key === "/" && !isInput) {
    e.preventDefault();
    if (searchInput) { searchInput.focus(); searchInput.select(); }
    return;
  }

  // g then another key for navigation (only when not in an input)
  if (!isInput) {
    if (e.key === "g" && !searchFocused) {
      searchFocused = true;
      var navTimer = setTimeout(function () { searchFocused = false; }, 1000);
      document.addEventListener(
        "keydown",
        function navHandler(ev) {
          if (ev.key === "l") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/libraries/"; }
          else if (ev.key === "s") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/starred/"; }
          else if (ev.key === "t") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/trash/"; }
          else if (ev.key === "a") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/activities/"; }
          else if (ev.key === "p") { clearTimeout(navTimer); searchFocused = false; window.location.href = "/profile/"; }
          else if (ev.key === "Escape") { clearTimeout(navTimer); searchFocused = false; }
          document.removeEventListener("keydown", navHandler);
        },
        { once: true }
      );
    }
  }
});

// ─── Star toggle (event delegation) ────────────────────────────────────
// The same hook is used by the always-present star in a file row and by the
// star in the details drawer. The row is the source of truth for `data-starred`
// (right-panel.js reads it when it renders the drawer), so mirror the new state
// onto the owning row — otherwise starring a file and then selecting it shows a
// stale state in the drawer.
function setStarState(btn, starred) {
  btn.dataset.starred = starred ? "true" : "false";
  btn.classList.toggle("on", starred);
  var svg = btn.querySelector("svg");
  if (svg) svg.setAttribute("fill", starred ? "currentColor" : "none");
  btn.title = starred ? __t('ui.unstar') : __t('ui.star');
  var row = btn.closest(".js-entry-row");
  if (row) {
    row.dataset.starred = starred ? "true" : "false";
    // Grid/gallery tiles keep their action chips visible while starred, so the
    // state is readable without hovering every tile.
    row.classList.toggle("starred", starred);
  }
}

document.addEventListener("click", async function (e) {
  const btn = e.target.closest("[data-toggle-star]");
  if (!btn) return;

  const repoId = btn.dataset.repoId;
  const path = btn.dataset.path;
  const currentlyStarred = btn.dataset.starred === "true";
  const csrfToken = getCookie("sfcsrftoken");
  if (!csrfToken) {
    window.location.href = "/accounts/login/";
    return;
  }

  btn.disabled = true;

  try {
    if (currentlyStarred) {
      const url =
        "/api/v2.1/starred-items/?repo_id=" +
        encodeURIComponent(repoId) +
        "&path=" +
        encodeURIComponent(path);
      const res = await fetch(url, {
        method: "DELETE",
        headers: { "X-CSRFToken": csrfToken },
      });
      if (res.ok) setStarState(btn, false);
    } else {
      const res = await fetch("/api/v2.1/starred-items/", {
        method: "POST",
        headers: {
          "X-CSRFToken": csrfToken,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ repo_id: repoId, path: path }),
      });
      if (res.ok) setStarState(btn, true);
    }
  } catch (ignored) {
    // Ignore network errors silently
  } finally {
    btn.disabled = false;
  }
});

// ─── Sidebar storage meter ─────────────────────────────────────────────
// GET /api2/account/info/ already reports `usage` and `total` in bytes
// (`total` is -1/0 when the quota is unlimited), so the meter needs no
// server-side plumbing into every page's template struct. The row itself is
// server-rendered (see includes/left_panel.html) so it holds its space from the
// first paint — this only fills in the text and the fill width, never the box.
(function () {
  var textEl = document.getElementById("nf-storage-text");
  var barEl = document.getElementById("nf-storage-bar");
  if (!textEl || !barEl) return;
  fetch("/api2/account/info/", { headers: { Accept: "application/json" } })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (info) {
      if (!info || typeof info.usage !== "number") return;
      var total = typeof info.total === "number" ? info.total : -1;
      if (total > 0) {
        textEl.textContent =
          formatFileSize(info.usage) + " / " + formatFileSize(total);
        var pct = Math.max(0, Math.min(100, Math.round((info.usage / total) * 100)));
        barEl.style.width = pct + "%";
      } else {
        // Unlimited quota — usage only, no bar to fill.
        textEl.textContent = formatFileSize(info.usage);
      }
    })
    .catch(function () { /* leave the row blank rather than resizing it */ });
})();
