// selection-state — pure selection reducer for the file browser (no DOM).
// state: { selected: Set<string>, anchor: string|null, touchMode: bool }.
// `anchor` stores the dataset.name of the last anchor row (used by Shift+click).

export function createSelectionState() {
  return { selected: new Set(), anchor: null, touchMode: false };
}

function toggle(selected, name) {
  var next = new Set(selected);
  if (next.has(name)) next.delete(name);
  else next.add(name);
  return next;
}

function reduceClick(state, action) {
  var name = action.name;
  var orderedNames = action.orderedNames || [];

  if (action.shift) {
    var anchorIdx = state.anchor != null ? orderedNames.indexOf(state.anchor) : -1;
    var currentIdx = orderedNames.indexOf(name);
    if (anchorIdx !== -1 && currentIdx !== -1) {
      var start = Math.min(anchorIdx, currentIdx);
      var end = Math.max(anchorIdx, currentIdx);
      var range = orderedNames.slice(start, end + 1).filter(function (n) { return n; });
      return { selected: new Set(range), anchor: state.anchor, touchMode: false };
    }
    // Anchor missing or not in the current view — fall back to single select.
    return { selected: new Set([name]), anchor: name, touchMode: false };
  }

  if (action.ctrl || state.touchMode) {
    return { selected: toggle(state.selected, name), anchor: state.anchor, touchMode: state.touchMode };
  }

  if (state.selected.size === 1 && state.selected.has(name)) {
    // Clicking the only selected item deselects it.
    return { selected: new Set(), anchor: name, touchMode: false };
  }
  return { selected: new Set([name]), anchor: name, touchMode: false };
}

export function reduceSelection(state, action) {
  switch (action.type) {
    case "click":
      return reduceClick(state, action);
    case "selectOne":
      // dblclick: clear then single-select (touch mode is cleared, anchor kept).
      return { selected: new Set([action.name]), anchor: state.anchor, touchMode: false };
    case "selectAll":
      return { selected: new Set(action.names), anchor: state.anchor, touchMode: state.touchMode };
    case "clear":
      // clearSelection clears touch mode but preserves the anchor.
      return { selected: new Set(), anchor: state.anchor, touchMode: false };
    case "setTouchMode":
      return { selected: state.selected, anchor: state.anchor, touchMode: action.value };
    default:
      return state;
  }
}
