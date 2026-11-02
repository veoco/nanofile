import { test } from "node:test";
import assert from "node:assert/strict";
import { tagLabel } from "./tag-label.js";

test("automatic tags are translated from their label key", () => {
  globalThis.window = {
    __T: { "fb.tag_person_any": "人物", "fb.tag_person_group": "合影" },
  };
  assert.equal(tagLabel({ name: "People", labelKey: "person_any" }), "人物");
  assert.equal(tagLabel({ name: "合影", labelKey: "person_group" }), "合影");
});

test("hand-made tags keep their name", () => {
  globalThis.window = { __T: { "fb.tag_person_any": "人物" } };
  assert.equal(tagLabel({ name: "work" }), "work");
  assert.equal(tagLabel({ name: "People" }), "People");
});

test("a missing translation falls back to the stored name", () => {
  globalThis.window = { __T: {} };
  assert.equal(tagLabel({ name: "People", labelKey: "person_any" }), "People");
});

test("a missing tag renders as an empty string", () => {
  assert.equal(tagLabel(null), "");
  assert.equal(tagLabel(undefined), "");
  assert.equal(tagLabel({}), "");
});
