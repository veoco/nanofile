import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { PREVIEW_FAILURE } from "./preview-failure.js";

// The module touches `document` only inside its functions, so importing it here
// needs no DOM.
// Every sentence a preview shows must exist in both dictionaries: a missing key
// renders as the key itself (`fb.media_unsupported`), which is the kind of
// breakage only a screenshot catches otherwise.
const LOCALES = ["en", "zh"];

test("every preview failure key is translated", function () {
  var keys = Object.keys(PREVIEW_FAILURE).map(function (kind) {
    return PREVIEW_FAILURE[kind];
  });
  keys.push("fb.download_instead");

  LOCALES.forEach(function (lang) {
    var toml = readFileSync(new URL("../../locales/" + lang + ".toml", import.meta.url), "utf8");
    keys.forEach(function (key) {
      var pattern = new RegExp("^" + key.replace(/\./g, "\\.") + "\\s*=", "m");
      assert.ok(pattern.test(toml), key + " is missing from " + lang + ".toml");
    });
  });
});

// The map is what the two surfaces look their wording up by; a kind that is not
// in it falls back to the generic sentence rather than rendering "undefined".
test("the failure map names a kind per reason", function () {
  assert.deepEqual(Object.keys(PREVIEW_FAILURE).sort(), ["image", "media", "text"]);
});
