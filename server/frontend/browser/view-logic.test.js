import { test } from "node:test";
import assert from "node:assert/strict";
import { applySortIndicators, nextSortOrder, nextTagFilter } from "./view-logic.js";

test("nextSortOrder toggles same field, resets new field", () => {
  assert.equal(nextSortOrder("name", "name", "asc"), "desc");
  assert.equal(nextSortOrder("name", "name", "desc"), "asc");
  assert.equal(nextSortOrder("size", "name", "desc"), "asc");
});

test("nextTagFilter toggles tag", () => {
  assert.equal(nextTagFilter("", "work"), "work");
  assert.equal(nextTagFilter("work", "work"), "");
  assert.equal(nextTagFilter("work", "home"), "home");
});

function fakeButton(sort, upFill, downFill) {
  var classes = {};
  return {
    dataset: { sort: sort },
    classList: {
      toggle: function (name, on) {
        classes[name] = on;
      },
    },
    isOn: function (name) {
      return classes[name] === true;
    },
    querySelector: function (selector) {
      if (selector === ".js-sort-arrow-up") return { style: { fill: upFill } };
      if (selector === ".js-sort-arrow-down") return { style: { fill: downFill } };
      return null;
    },
  };
}

test("applySortIndicators lights the active field and direction", () => {
  var up = { style: { fill: "" } };
  var down = { style: { fill: "" } };
  var nameBtn = {
    dataset: { sort: "name" },
    classList: { toggle: function () {} },
    querySelector: function (selector) {
      return selector === ".js-sort-arrow-up" ? up : down;
    },
  };
  var sizeBtn = fakeButton("size", "", "");
  var root = {
    querySelectorAll: function () {
      return [nameBtn, sizeBtn];
    },
  };

  applySortIndicators(root, "name", "desc");
  assert.equal(up.style.fill, "var(--color-ink-3)");
  assert.equal(down.style.fill, "var(--color-accent)");
  assert.equal(sizeBtn.isOn("on"), false);
});