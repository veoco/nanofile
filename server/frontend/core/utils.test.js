import { test } from "node:test";
import assert from "node:assert/strict";
import { encodeFilePath, unquote, safeColor, escapeAttr } from "./utils.js";

test("encodeFilePath preserves slashes and encodes each segment", () => {
  assert.equal(encodeFilePath("a/b/c"), "a/b/c");
  assert.equal(encodeFilePath("a b/c d"), "a%20b/c%20d");
  assert.equal(encodeFilePath("/foo/bar"), "/foo/bar");
  assert.equal(
    encodeFilePath("文档 1/报告.pdf"),
    encodeURIComponent("文档 1") + "/" + encodeURIComponent("报告.pdf"),
  );
});

test("unquote strips surrounding double quotes", () => {
  assert.equal(unquote('"foo"'), "foo");
  assert.equal(unquote("foo"), "foo");
  assert.equal(unquote('"foo'), "foo");
  assert.equal(unquote('foo"'), "foo");
  assert.equal(unquote('""'), "");
  assert.equal(unquote(""), "");
});

test("safeColor accepts valid hex and rejects anything else", () => {
  assert.equal(safeColor("#abc"), "#abc");
  assert.equal(safeColor("#A1B2C3"), "#A1B2C3");
  assert.equal(safeColor("#abcd"), "#abcd");
  assert.equal(safeColor("red"), "#e6e6e6");
  assert.equal(safeColor("red", "#000000"), "#000000");
  assert.equal(safeColor(null), "#e6e6e6");
  assert.equal(safeColor(""), "#e6e6e6");
  assert.equal(safeColor("#12"), "#e6e6e6");
  assert.equal(safeColor("#123456789"), "#e6e6e6");
});

test("escapeAttr escapes & \" ' < >", () => {
  assert.equal(escapeAttr('a&b<c>d"e\'f'), "a&amp;b&lt;c&gt;d&quot;e&#39;f");
  assert.equal(escapeAttr(null), "");
  assert.equal(escapeAttr("plain"), "plain");
});
