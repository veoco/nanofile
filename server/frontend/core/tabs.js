// tabs — tab switching for the pages that render a tab bar (/shares/,
// /sysadmin/shares/, /trash/). Tabs are plain buttons marked `role="tab"` +
// `aria-selected`; the active look is a CSS rule on that attribute, so nothing
// here knows about styling. Every `.tab-content` is toggled from the `tab`
// query parameter, so switching updates the URL without a navigation and a
// reload keeps the user on the same tab.
//
// `defaultTab` is the tab that means "no `tab` parameter" for that page.

export function switchTab(name, defaultTab) {
  document.querySelectorAll(".tab-content").forEach(function (el) {
    el.classList.add("hidden");
  });
  document.querySelectorAll(".nf-tab").forEach(function (el) {
    el.setAttribute("aria-selected", "false");
    el.tabIndex = -1;
  });

  var content = document.getElementById("tab-" + name);
  if (content) content.classList.remove("hidden");

  var btn = document.querySelector('.nf-tab[data-tab="' + name + '"]');
  if (btn) {
    btn.setAttribute("aria-selected", "true");
    btn.tabIndex = 0;
  }

  var params = new URLSearchParams(window.location.search);
  if (name === defaultTab) params.delete("tab");
  else params.set("tab", name);
  var newSearch = params.toString();
  window.history.replaceState(
    null,
    "",
    newSearch ? window.location.pathname + "?" + newSearch : window.location.pathname
  );
}

// Left/Right move between tabs, which `role="tab"` promises. Delegated rather
// than registered per tab bar, and it re-dispatches a click so selecting a tab
// keeps the single code path the page bundles already own.
document.addEventListener("keydown", function (e) {
  if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
  var tab = e.target.closest ? e.target.closest('.nf-tab[data-tab]') : null;
  if (!tab) return;
  var tabs = Array.prototype.slice.call(
    document.querySelectorAll(".nf-tab[data-tab]")
  );
  var i = tabs.indexOf(tab);
  if (i < 0) return;
  var step = e.key === "ArrowRight" ? 1 : -1;
  var next = tabs[(i + step + tabs.length) % tabs.length];
  if (!next) return;
  e.preventDefault();
  next.focus();
  next.click();
});
