// Nanofile Web UI — main.js
(function () {
  "use strict";

  // Mobile sidebar toggle
  const menuToggle = document.querySelector(".js-mobile-menu-toggle");
  const sidebar = document.querySelector(".js-sidebar");
  const overlay = document.querySelector(".js-sidebar-overlay");

  function openSidebar() {
    if (sidebar) sidebar.classList.remove("hidden");
    if (sidebar) sidebar.classList.add("flex");
    if (overlay) overlay.classList.remove("hidden");
  }

  function closeSidebar() {
    if (sidebar) sidebar.classList.remove("flex");
    if (sidebar) sidebar.classList.add("hidden");
    if (overlay) overlay.classList.add("hidden");
  }

  if (menuToggle) {
    menuToggle.addEventListener("click", function (e) {
      e.stopPropagation();
      if (sidebar && sidebar.classList.contains("hidden")) {
        openSidebar();
      } else {
        closeSidebar();
      }
    });
  }

  if (overlay) {
    overlay.addEventListener("click", closeSidebar);
  }

  // Close sidebar on Escape key
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") closeSidebar();
  });

  // User menu dropdown
  const userMenu = document.querySelector(".js-user-menu");
  const userButton = document.querySelector(".js-user-menu-button");
  if (userMenu && userButton) {
    userButton.addEventListener("click", function (e) {
      e.stopPropagation();
      let dropdown = userMenu.querySelector(".js-user-menu-dropdown");
      if (dropdown) {
        dropdown.remove();
        return;
      }
      dropdown = document.createElement("div");
      dropdown.className =
        "js-user-menu-dropdown absolute right-0 z-50 mt-2 w-48 origin-top-right rounded-md bg-white py-1 shadow-lg ring-1 ring-black ring-opacity-5 focus:outline-none";

      const links = [
        { href: "/profile/", text: "Settings" },
        { href: "/accounts/logout/", text: "Sign out" },
      ];
      for (const link of links) {
        const a = document.createElement("a");
        a.href = link.href;
        a.className = "block px-4 py-2 text-sm text-gray-700 hover:bg-gray-100";
        a.textContent = link.text;
        dropdown.appendChild(a);
      }

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

  // Dark mode toggle
  const darkToggle = document.querySelector(".js-dark-toggle");
  if (darkToggle) {
    darkToggle.addEventListener("click", function () {
      document.documentElement.classList.toggle("dark");
      localStorage.setItem(
        "darkMode",
        document.documentElement.classList.contains("dark")
      );
    });
    // Restore preference
    if (localStorage.getItem("darkMode") === "true") {
      document.documentElement.classList.add("dark");
    }
  }

  // ── Star toggle (event delegation) ─────────────────────────────────────
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
        // DELETE — unstar (uses X-CSRFToken header + session cookie)
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
          btn.classList.remove("text-yellow-400");
          btn.classList.add("text-gray-300", "hover:text-yellow-400");
          btn.querySelector("svg").setAttribute("fill", "none");
          btn.title = "Star";
          btn.dataset.starred = "false";
        }
      } else {
        // POST — star
        const res = await fetch("/api/v2.1/starred-items/", {
          method: "POST",
          headers: {
            "X-CSRFToken": csrfToken,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ repo_id: repoId, path: path }),
        });
        if (res.ok) {
          btn.classList.remove("text-gray-300", "hover:text-yellow-400");
          btn.classList.add("text-yellow-400");
          btn.querySelector("svg").setAttribute("fill", "currentColor");
          btn.title = "Unstar";
          btn.dataset.starred = "true";
        }
      }
    } catch (ignored) {
      // Ignore network errors silently
    } finally {
      btn.disabled = false;
    }
  });

  // ── Read a cookie value by name ────────────────────────────────────────
  function getCookie(name) {
    const match = document.cookie.match(
      "(^|;)\\s*" + name + "\\s*=\\s*([^;]+)"
    );
    return match ? match.pop() : "";
  }

  // ── Delete confirmation (event delegation) ────────────────────────────
  document.addEventListener("submit", function (e) {
    const form = e.target.closest(".js-delete-form");
    if (!form) return;

    const nameInput = form.querySelector('input[name="name"]');
    const name = nameInput ? nameInput.value : "";
    const msg = 'Delete "' + name + '"? This cannot be undone.';
    if (!confirm(msg)) {
      e.preventDefault();
    }
  });

  // ── Rename dialog ─────────────────────────────────────────────────────
  const renameOverlay = document.getElementById("rename-overlay");
  const renameOldPath = document.getElementById("rename-old-path");
  const renameInput = document.getElementById("rename-input");

  // Open rename dialog
  document.addEventListener("click", function (e) {
    const btn = e.target.closest(".js-rename-btn");
    if (!btn) return;

    renameOldPath.value = btn.dataset.path;
    renameInput.value = btn.dataset.name;
    renameOverlay.classList.remove("hidden");
    setTimeout(function () {
      renameInput.focus();
      renameInput.select();
    }, 100);
  });

  // Close rename dialog on overlay background click
  if (renameOverlay) {
    renameOverlay.addEventListener("click", function (e) {
      if (e.target === renameOverlay) {
        renameOverlay.classList.add("hidden");
      }
    });
  }

  // Close rename dialog on Escape
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape" && renameOverlay && !renameOverlay.classList.contains("hidden")) {
      renameOverlay.classList.add("hidden");
    }
  });

  // Close rename dialog on Cancel
  const renameCancel = document.querySelector(".js-rename-cancel");
  if (renameCancel) {
    renameCancel.addEventListener("click", function () {
      renameOverlay.classList.add("hidden");
    });
  }

  // Rename form submit
  const renameForm = document.getElementById("rename-dialog-form");
  if (renameForm) {
    renameForm.addEventListener("submit", function (e) {
      var newName = renameInput.value.trim();
      if (!newName) {
        e.preventDefault();
        return;
      }
      renameOverlay.classList.add("hidden");
    });
  }

  // ── Trash restore confirmation (event delegation) ───────────────────
  document.addEventListener("submit", function (e) {
    const form = e.target.closest(".js-restore-form");
    if (!form) return;

    const objName = form.dataset.objName || "";
    const repoName = form.dataset.repoName || "";
    const msg = 'Restore "' + objName + '" from ' + repoName + "?";
    if (!confirm(msg)) {
      e.preventDefault();
    }
  });

  // ── Repo delete confirmation (event delegation) ────────────────────
  document.addEventListener("submit", function (e) {
    const form = e.target.closest(".js-delete-repo-form");
    if (!form) return;

    const repoName = form.dataset.repoName || "";
    const msg = 'Delete library "' + repoName + '"? All files will be permanently lost.';
    if (!confirm(msg)) {
      e.preventDefault();
    }
  });
})();
