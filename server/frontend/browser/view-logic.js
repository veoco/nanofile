// view-logic — pure sort/tag toggle helpers for the view toolbar (no DOM).

// Clicking a sort field: same field toggles asc/desc, a new field resets to asc.
export function nextSortOrder(field, currentField, currentOrder) {
  return field === currentField ? (currentOrder === "asc" ? "desc" : "asc") : "asc";
}

// Clicking a tag filter: same tag clears the filter, a new tag selects it.
export function nextTagFilter(current, name) {
  return current === name ? "" : name;
}

// Mark the active sort button and light the arrow for the current direction.
//
// Both sort bars (file list and library list) had a copy of this loop; only the
// button/arrow selectors differ, so they pass them in.
export function applySortIndicators(root, field, order, options) {
  if (!root) return;
  var opts = options || {};
  var btnSelector = opts.btnSelector || ".js-sort-btn";
  var upSelector = opts.upSelector || ".js-sort-arrow-up";
  var downSelector = opts.downSelector || ".js-sort-arrow-down";
  var activeClass = opts.activeClass || "on";
  var activeFill = opts.activeFill || "var(--color-accent)";
  var inactiveFill = opts.inactiveFill || "var(--color-ink-3)";
  var buttons = root.querySelectorAll(btnSelector);
  for (var i = 0; i < buttons.length; i++) {
    var btn = buttons[i];
    var active = btn.dataset.sort === field;
    btn.classList.toggle(activeClass, active);
    var up = btn.querySelector(upSelector);
    var down = btn.querySelector(downSelector);
    if (up) up.style.fill = active && order === "asc" ? activeFill : inactiveFill;
    if (down) down.style.fill = active && order === "desc" ? activeFill : inactiveFill;
  }
}