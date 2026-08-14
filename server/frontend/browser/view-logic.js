// view-logic — pure sort/tag toggle helpers for the view toolbar (no DOM).

// Clicking a sort field: same field toggles asc/desc, a new field resets to asc.
export function nextSortOrder(field, currentField, currentOrder) {
  return field === currentField ? (currentOrder === "asc" ? "desc" : "asc") : "asc";
}

// Clicking a tag filter: same tag clears the filter, a new tag selects it.
export function nextTagFilter(current, name) {
  return current === name ? "" : name;
}
