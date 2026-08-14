import { test } from "node:test";
import assert from "node:assert/strict";
import { __t } from "./i18n.js";

test("__t returns translation and substitutes placeholders", () => {
  globalThis.window = {
    __T: {
      "fb.greet": "Hello, {name}!",
      "fb.sum": "{a}+{b}={c}",
    },
  };
  assert.equal(__t("fb.greet", { name: "World" }), "Hello, World!");
  assert.equal(__t("fb.sum", { a: 1, b: 2, c: 3 }), "1+2=3");
});

test("__t falls back to the key itself when missing", () => {
  globalThis.window = { __T: {} };
  assert.equal(__t("missing.key"), "missing.key");
});

test("__t returns the translation as-is when no args", () => {
  globalThis.window = { __T: { "k": "value" } };
  assert.equal(__t("k"), "value");
});

test("__t leaves placeholders without a matching arg untouched", () => {
  globalThis.window = { __T: { "k": "{x} and {y}" } };
  assert.equal(__t("k", { x: "1" }), "1 and {y}");
});
