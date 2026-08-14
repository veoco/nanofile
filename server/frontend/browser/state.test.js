import { test } from "node:test";
import assert from "node:assert/strict";
import { getSort, getTagFilter, getVisibleView, getVisibleViewContainer } from "./state.js";

function makeEl({ dataset = {}, hidden = false } = {}) {
  return { dataset, classList: { contains: (c) => c === "hidden" && hidden } };
}

function installDom(map) {
  globalThis.document = {
    querySelector: (sel) =>
      Object.prototype.hasOwnProperty.call(map, sel) ? map[sel] : null,
  };
}

test("getSort reads sort bar dataset", () => {
  installDom({ ".js-sort-bar": makeEl({ dataset: { sortField: "size", sortOrder: "desc" } }) });
  assert.deepEqual(getSort(), { sort: "size", sort_order: "desc" });
});

test("getSort falls back to name/asc when dataset empty", () => {
  installDom({ ".js-sort-bar": makeEl() });
  assert.deepEqual(getSort(), { sort: "name", sort_order: "asc" });
});

test("getSort falls back to localStorage when no sort bar", () => {
  installDom({});
  globalThis.localStorage = {
    getItem: (k) => (k === "fileSortField" ? "mtime" : "asc"),
  };
  assert.deepEqual(getSort(), { sort: "mtime", sort_order: "asc" });
});

test("getSort defaults when localStorage empty", () => {
  installDom({});
  globalThis.localStorage = { getItem: () => null };
  assert.deepEqual(getSort(), { sort: "name", sort_order: "asc" });
});

test("getTagFilter returns tag from sort bar or empty", () => {
  installDom({ ".js-sort-bar": makeEl({ dataset: { tagFilter: "work" } }) });
  assert.equal(getTagFilter(), "work");

  installDom({});
  assert.equal(getTagFilter(), "");
});

test("getVisibleView returns gallery/grid/list", () => {
  installDom({
    ".js-gallery-view": makeEl(),
    ".js-file-grid-view": makeEl({ hidden: true }),
  });
  assert.equal(getVisibleView(), "gallery");

  installDom({
    ".js-gallery-view": makeEl({ hidden: true }),
    ".js-file-grid-view": makeEl(),
  });
  assert.equal(getVisibleView(), "grid");

  installDom({
    ".js-gallery-view": makeEl({ hidden: true }),
    ".js-file-grid-view": makeEl({ hidden: true }),
  });
  assert.equal(getVisibleView(), "list");
});

test("getVisibleViewContainer returns the visible container element", () => {
  var gallery = makeEl();
  installDom({
    ".js-gallery-view": gallery,
    ".js-file-grid-view": makeEl({ hidden: true }),
  });
  assert.equal(getVisibleViewContainer(), gallery);

  var list = makeEl();
  installDom({
    ".js-gallery-view": makeEl({ hidden: true }),
    ".js-file-grid-view": makeEl({ hidden: true }),
    ".js-file-list-view": list,
  });
  assert.equal(getVisibleViewContainer(), list);
});
