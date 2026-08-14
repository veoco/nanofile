import { test } from "node:test";
import assert from "node:assert/strict";
import { createSelectionState, reduceSelection } from "./selection-state.js";

var NAMES = ["a", "b", "c", "d"];

function click(state, name, opts) {
  return reduceSelection(state, {
    type: "click",
    name: name,
    shift: !!(opts && opts.shift),
    ctrl: !!(opts && opts.ctrl),
    orderedNames: (opts && opts.orderedNames) || NAMES,
  });
}

test("createSelectionState starts empty", () => {
  var s = createSelectionState();
  assert.deepEqual(s.selected, new Set());
  assert.equal(s.anchor, null);
  assert.equal(s.touchMode, false);
});

test("plain click selects a single row", () => {
  var s = click(createSelectionState(), "a");
  assert.deepEqual(s.selected, new Set(["a"]));
  assert.equal(s.anchor, "a");
});

test("plain click on another row switches selection", () => {
  var s = click(click(createSelectionState(), "a"), "b");
  assert.deepEqual(s.selected, new Set(["b"]));
  assert.equal(s.anchor, "b");
});

test("plain click on the only selected row deselects it", () => {
  var s = click(click(createSelectionState(), "a"), "a");
  assert.deepEqual(s.selected, new Set());
  assert.equal(s.anchor, "a");
});

test("ctrl click toggles a row on and off", () => {
  var s = click(click(createSelectionState(), "a"), "b", { ctrl: true });
  assert.deepEqual(s.selected, new Set(["a", "b"]));
  s = click(s, "a", { ctrl: true });
  assert.deepEqual(s.selected, new Set(["b"]));
});

test("touch mode toggles like ctrl", () => {
  var s = reduceSelection(createSelectionState(), { type: "setTouchMode", value: true });
  s = click(s, "a");
  s = click(s, "b");
  assert.deepEqual(s.selected, new Set(["a", "b"]));
  assert.equal(s.touchMode, true);
});

test("shift click selects a range forward", () => {
  var s = click(click(createSelectionState(), "b"), "d", { shift: true });
  assert.deepEqual(s.selected, new Set(["b", "c", "d"]));
  assert.equal(s.anchor, "b");
});

test("shift click selects a range backward", () => {
  var s = click(click(createSelectionState(), "c"), "a", { shift: true });
  assert.deepEqual(s.selected, new Set(["a", "b", "c"]));
  assert.equal(s.anchor, "c");
});

test("shift click without anchor falls back to single select", () => {
  var s = click(createSelectionState(), "c", { shift: true });
  assert.deepEqual(s.selected, new Set(["c"]));
  assert.equal(s.anchor, "c");
});

test("selectOne clears and selects a single row", () => {
  var s = click(createSelectionState(), "a");
  s = reduceSelection(s, { type: "selectOne", name: "b" });
  assert.deepEqual(s.selected, new Set(["b"]));
});

test("selectAll selects all names", () => {
  var s = reduceSelection(createSelectionState(), { type: "selectAll", names: ["a", "b", "c"] });
  assert.deepEqual(s.selected, new Set(["a", "b", "c"]));
});

test("clear empties selection and resets touch mode", () => {
  var s = reduceSelection(createSelectionState(), { type: "setTouchMode", value: true });
  s = reduceSelection(s, { type: "selectAll", names: ["a", "b"] });
  s = reduceSelection(s, { type: "clear" });
  assert.deepEqual(s.selected, new Set());
  assert.equal(s.touchMode, false);
});

test("setTouchMode toggles the flag", () => {
  var s = reduceSelection(createSelectionState(), { type: "setTouchMode", value: true });
  assert.equal(s.touchMode, true);
  s = reduceSelection(s, { type: "setTouchMode", value: false });
  assert.equal(s.touchMode, false);
});

test("unknown action returns state unchanged", () => {
  var s = createSelectionState();
  var out = reduceSelection(s, { type: "nope" });
  assert.equal(out, s);
});
