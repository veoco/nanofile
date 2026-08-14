import { test } from "node:test";
import assert from "node:assert/strict";
import { encodeFilePath, unquote, safeColor, escapeAttr, escapeHtml, getCookie, parentDirOf } from "./utils.js";

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

function installFakeDom() {
  var currentText = "";
  var div = {
    appendChild: function (node) {
      currentText = node.text;
    },
    get innerHTML() {
      return currentText
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;");
    },
  };
  globalThis.document = {
    createElement: function () {
      return div;
    },
    createTextNode: function (s) {
      return { text: s };
    },
  };
}

test("escapeHtml escapes & < >", () => {
  installFakeDom();
  assert.equal(escapeHtml("a&b<c>d"), "a&amp;b&lt;c&gt;d");
});

test("escapeHtml handles null/undefined and stringifies", () => {
  installFakeDom();
  assert.equal(escapeHtml(null), "");
  assert.equal(escapeHtml(undefined), "");
  assert.equal(escapeHtml(123), "123");
});

test("getCookie extracts a value", () => {
  globalThis.document = { cookie: "a=1; sfcsrftoken=abc; b=2" };
  assert.equal(getCookie("sfcsrftoken"), "abc");
});

test("getCookie returns empty when missing or empty", () => {
  globalThis.document = { cookie: "a=1" };
  assert.equal(getCookie("sfcsrftoken"), "");
  globalThis.document = { cookie: "" };
  assert.equal(getCookie("x"), "");
});

test("parentDirOf extracts the parent directory", () => {
  assert.equal(parentDirOf("/a/b/c"), "/a/b");
  assert.equal(parentDirOf("/a"), "/");
  assert.equal(parentDirOf("/"), "/");
  assert.equal(parentDirOf("a"), "/");
});
