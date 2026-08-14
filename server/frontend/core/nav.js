// nav — global chrome interactions: mobile panel, user menu, dark mode,
// quick search, keyboard shortcuts, and the star toggle. Loaded once for all
// pages (side effects only).
import { __t } from "./i18n.js";
import { getCookie } from "./utils.js";

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
      "js-user-menu-dropdown absolute right-0 z-50 mt-2 w-44 origin-top-right rounded-xl bg-white dark:bg-surface-800 py-1 shadow-lg ring-1 ring-black/5 dark:ring-white/10 focus:outline-none";

    // Admin-only: User Management link
    var isAdmin = userMenu.getAttribute("data-is-admin") === "true";
    if (isAdmin) {
      var adminLink = document.createElement("a");
      adminLink.href = "/sysadmin/users/";
      adminLink.className = "block px-4 py-2 text-sm text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-surface-700";
      adminLink.textContent = __t('ui.user_management');
      dropdown.appendChild(adminLink);

      var shareLink = document.createElement("a");
      shareLink.href = "/sysadmin/shares/";
      shareLink.className = "block px-4 py-2 text-sm text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-surface-700";
      shareLink.textContent = __t('ui.share_management');
      dropdown.appendChild(shareLink);

      var taskLink = document.createElement("a");
      taskLink.href = "/sysadmin/tasks/";
      taskLink.className = "block px-4 py-2 text-sm text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-surface-700";
      taskLink.textContent = __t('ui.task_management');
      dropdown.appendChild(taskLink);
    }

    var signOut = document.createElement("a");
    signOut.href = "/accounts/logout/";
    signOut.className = "block px-4 py-2 text-sm text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-surface-700";
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
      if (res.ok) {
        btn.classList.remove("text-yellow-400", "text-amber-400");
        btn.classList.add("text-gray-300", "hover:text-amber-400", "dark:text-gray-600");
        btn.querySelector("svg").setAttribute("fill", "none");
        btn.title = __t('ui.star');
        btn.dataset.starred = "false";
      }
    } else {
      const res = await fetch("/api/v2.1/starred-items/", {
        method: "POST",
        headers: {
          "X-CSRFToken": csrfToken,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ repo_id: repoId, path: path }),
      });
      if (res.ok) {
        btn.classList.remove("text-gray-300", "hover:text-amber-400", "dark:text-gray-600");
        btn.classList.add("text-amber-400");
        btn.querySelector("svg").setAttribute("fill", "currentColor");
        btn.title = __t('ui.unstar');
        btn.dataset.starred = "true";
      }
    }
  } catch (ignored) {
    // Ignore network errors silently
  } finally {
    btn.disabled = false;
  }
});
