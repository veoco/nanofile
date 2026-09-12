// tabs — tab switching for the pages that render a tab bar (/shares/,
// /sysadmin/shares/, /trash/). Tabs are plain buttons: every `.tab-content` is
// toggled here and the server renders the active one from the `tab` query
// parameter, so switching updates the URL without a navigation and a reload
// keeps the user on the same tab.
//
// `defaultTab` is the tab that means "no `tab` parameter" for that page.

export function switchTab(name, defaultTab) {
  document.querySelectorAll(".tab-content").forEach(function (el) {
    el.classList.add("hidden");
  });
  document.querySelectorAll(".tab-btn").forEach(function (el) {
    el.classList.remove(
      "tab-btn--active",
      "text-brand-600",
      "dark:text-brand-400",
      "border-brand-600",
      "dark:border-brand-400"
    );
    el.classList.add("text-gray-500", "dark:text-gray-400", "border-transparent");
  });

  var content = document.getElementById("tab-" + name);
  if (content) content.classList.remove("hidden");

  var btn = document.querySelector('[data-tab="' + name + '"]');
  if (btn) {
    btn.classList.remove("text-gray-500", "dark:text-gray-400", "border-transparent");
    btn.classList.add(
      "tab-btn--active",
      "text-brand-600",
      "dark:text-brand-400",
      "border-brand-600",
      "dark:border-brand-400"
    );
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
