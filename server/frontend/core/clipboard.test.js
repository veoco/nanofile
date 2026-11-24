import { test } from "node:test";
import assert from "node:assert/strict";
import { copyText, flashCopied } from "./clipboard.js";

function installClipboard(writeText) {
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: writeText ? { clipboard: { writeText } } : {},
  });
}

test("copyText uses the async clipboard when it resolves", async () => {
  var seen = "";
  installClipboard(async function (value) {
    seen = value;
  });
  assert.equal(await copyText("hello"), true);
  assert.equal(seen, "hello");
});

test("copyText falls back when the async clipboard rejects", async () => {
  installClipboard(async function () {
    throw new Error("denied");
  });
  var selected = "";
  globalThis.document = {
    body: { appendChild: function () {}, removeChild: function () {} },
    createElement: function () {
      return {
        value: "",
        style: {},
        setAttribute: function () {},
        select: function () {
          selected = this.value;
        },
      };
    },
    execCommand: function () {
      return true;
    },
  };
  assert.equal(await copyText("fallback"), true);
  assert.equal(selected, "fallback");
});

test("copyText reports failure when the fallback cannot copy", async () => {
  installClipboard(null);
  globalThis.document = {
    body: { appendChild: function () {}, removeChild: function () {} },
    createElement: function () {
      return { style: {}, setAttribute: function () {}, select: function () {} };
    },
    execCommand: function () {
      return false;
    },
  };
  assert.equal(await copyText("nope"), false);
});

test("copyText ignores empty values", async () => {
  installClipboard(async function () {
    throw new Error("must not be called");
  });
  assert.equal(await copyText(""), false);
});

test("flashCopied restores the original label", () => {
  var button = { textContent: "Copy" };
  flashCopied(button, "Copied", 10);
  assert.equal(button.textContent, "Copied");
  return new Promise(function (resolve) {
    setTimeout(function () {
      assert.equal(button.textContent, "Copy");
      resolve();
    }, 30);
  });
});
