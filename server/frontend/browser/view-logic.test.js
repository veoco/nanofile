import { test } from "node:test";
import assert from "node:assert/strict";
import { nextSortOrder, nextTagFilter } from "./view-logic.js";

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
