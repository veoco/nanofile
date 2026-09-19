// Pre-paint preferences.
//
// These must run before the first paint, otherwise the page flashes the wrong
// theme / view mode. They are loaded as a plain blocking script in <head>
// instead of being inlined, so the Content-Security-Policy can drop
// 'unsafe-inline' from script-src.
(function () {
  // The canvas colour also has to be painted before app.css arrives, and this
  // script already gates that first paint. Setting it here (rather than in an
  // inline <style>) keeps `--color-canvas` the single source of truth: the
  // literals below mirror the light and dark values in input.css, and the
  // inline property is dropped again once the stylesheet has been applied.
  var root = document.documentElement;
  function paintCanvas() {
    root.style.backgroundColor = root.classList.contains("dark") ? "#111111" : "#fcfcfc";
  }
  try {
    if (localStorage.getItem("darkMode") === "true") {
      root.classList.add("dark");
    }
    var view = localStorage.getItem("fileViewMode");
    if (view !== "grid" && view !== "gallery") view = "list";
    // Always publish the view mode: the CSS marking the active view-switcher
    // button is keyed on `html[data-view]`, so an unset attribute would leave
    // the switcher with no active state at all.
    root.dataset.view = view;
  } catch (e) {
    // localStorage can be unavailable (private mode, blocked cookies); the
    // defaults are fine, so never let this break the page.
    root.dataset.view = "list";
  }
  paintCanvas();
  window.addEventListener("load", function () {
    root.removeAttribute("style");
  });
})();
