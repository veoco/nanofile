// Pre-paint preferences.
//
// These must run before the first paint, otherwise the page flashes the wrong
// theme / view mode. They are loaded as a plain blocking script in <head>
// instead of being inlined, so the Content-Security-Policy can drop
// 'unsafe-inline' from script-src.
(function () {
  try {
    if (localStorage.getItem("darkMode") === "true") {
      document.documentElement.classList.add("dark");
    }
    var view = localStorage.getItem("fileViewMode");
    if (view === "grid" || view === "gallery") {
      document.documentElement.dataset.view = view;
    }
  } catch (e) {
    // localStorage can be unavailable (private mode, blocked cookies); the
    // defaults are fine, so never let this break the page.
  }
})();
