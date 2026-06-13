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
  // Star file/directory toggle (global, called from onclick in file_list.html)
  window.toggleStar = async function (repoId, path, currentlyStarred, btn) {
    // Read session token from cookie
    function getCookie(name) {
      const match = document.cookie.match(new RegExp("(^| )" + name + "=([^;]+)"));
      return match ? match[2] : "";
    }
    const token = getCookie("seahub-session");
    if (!token) {
      window.location.href = "/accounts/login/";
      return;
    }

    btn.disabled = true;

    try {
      if (currentlyStarred) {
        // DELETE — unstar
        const url =
          "/api/v2.1/starred-items/?repo_id=" +
          encodeURIComponent(repoId) +
          "&path=" +
          encodeURIComponent(path);
        const res = await fetch(url, {
          method: "DELETE",
          headers: { Authorization: "Token " + token },
        });
        if (res.ok) {
          btn.classList.remove("text-yellow-400");
          btn.classList.add("text-gray-300", "hover:text-yellow-400");
          btn.querySelector("svg").setAttribute("fill", "none");
          btn.title = "Star";
          btn.setAttribute(
            "onclick",
            "toggleStar('" +
              repoId +
              "', '" +
              path.replace(/'/g, "\\'") +
              "', false, this)"
          );
        }
      } else {
        // POST — star
        const res = await fetch("/api/v2.1/starred-items/", {
          method: "POST",
          headers: {
            Authorization: "Token " + token,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ repo_id: repoId, path: path }),
        });
        if (res.ok) {
          btn.classList.remove("text-gray-300", "hover:text-yellow-400");
          btn.classList.add("text-yellow-400");
          btn.querySelector("svg").setAttribute("fill", "currentColor");
          btn.title = "Unstar";
          btn.setAttribute(
            "onclick",
            "toggleStar('" +
              repoId +
              "', '" +
              path.replace(/'/g, "\\'") +
              "', true, this)"
          );
        }
      }
    } catch (e) {
      // Ignore network errors silently — the UI stays in its current state
    } finally {
      btn.disabled = false;
    }
  };
})();

