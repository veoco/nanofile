// Pre-paint preferences: these must run before the first paint or the page flashes the wrong
// theme/view mode. They are a blocking external script rather than inline so CSP can drop
// 'unsafe-inline' from script-src.
(function () {
// The canvas colour must also be painted before app.css arrives. Setting it here keeps
// `--color-canvas` the single source of truth: the literals mirror input.css, and the inline
// property is dropped once the stylesheet applies.
  var root = document.documentElement;
  function paintCanvas() {
    root.style.backgroundColor = root.classList.contains("dark") ? "#111111" : "#fcfcfc";
  }
// Publish that scripts run at all. CSS holds back a block whose geometry only exists after the
// common bundle (the activity feed's day bands) on this attribute. A reader without JS never
// gets it, so nothing held back this way can be lost; set it before the preference reads, which
// may throw.
  root.dataset.js = "1";
  try {
    if (localStorage.getItem("darkMode") === "true") {
      root.classList.add("dark");
    }
    var view = localStorage.getItem("fileViewMode");
    if (view !== "grid" && view !== "gallery") view = "list";
// Always publish the view mode: the active-button CSS is keyed on `html[data-view]`.
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
