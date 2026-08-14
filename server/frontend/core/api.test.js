import { test } from "node:test";
import assert from "node:assert/strict";
import { apiFetch } from "./api.js";

function okRes(overrides) {
  return Object.assign(
    { ok: true, statusText: "OK", text: async () => "body" },
    overrides,
  );
}

test("injects CSRF token from cookie when header absent", async () => {
  globalThis.document = { cookie: "sfcsrftoken=abc123; other=x" };
  var captured;
  globalThis.fetch = async (url, options) => {
    captured = { url, options };
    return okRes();
  };

  var res = await apiFetch("/api/x");

  assert.equal(captured.url, "/api/x");
  assert.equal(captured.options.headers["X-CSRFToken"], "abc123");
  assert.equal(res.ok, true);
});

test("keeps an existing X-CSRFToken header", async () => {
  globalThis.document = { cookie: "sfcsrftoken=abc123" };
  var captured;
  globalThis.fetch = async (url, options) => {
    captured = options;
    return okRes();
  };

  await apiFetch("/api/x", { headers: { "X-CSRFToken": "custom" } });

  assert.equal(captured.headers["X-CSRFToken"], "custom");
});

test("sets empty CSRF when cookie missing", async () => {
  globalThis.document = { cookie: "" };
  var captured;
  globalThis.fetch = async (url, options) => {
    captured = options;
    return okRes();
  };

  await apiFetch("/api/x");
  assert.equal(captured.headers["X-CSRFToken"], "");
});

test("injects JSON content-type for string body without one", async () => {
  globalThis.document = { cookie: "" };
  var captured;
  globalThis.fetch = async (url, options) => {
    captured = options;
    return okRes();
  };

  await apiFetch("/api/x", { method: "POST", body: JSON.stringify({ a: 1 }) });
  assert.equal(captured.headers["Content-Type"], "application/json;charset=utf-8");
});

test("does not override an existing Content-Type", async () => {
  globalThis.document = { cookie: "" };
  var captured;
  globalThis.fetch = async (url, options) => {
    captured = options;
    return okRes();
  };

  await apiFetch("/api/x", {
    method: "POST",
    body: "a=b",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
  });
  assert.equal(captured.headers["Content-Type"], "application/x-www-form-urlencoded");
});

test("does not inject content-type for FormData body", async () => {
  globalThis.document = { cookie: "" };
  var captured;
  globalThis.fetch = async (url, options) => {
    captured = options;
    return okRes();
  };

  await apiFetch("/api/x", { method: "POST", body: new FormData() });
  assert.equal(captured.headers["Content-Type"], undefined);
});

test("throws Error with body text on non-ok response", async () => {
  globalThis.document = { cookie: "" };
  globalThis.fetch = async () => okRes({ ok: false, text: async () => "boom" });

  await assert.rejects(
    () => apiFetch("/api/x"),
    (err) => {
      assert.ok(err instanceof Error);
      assert.equal(err.message, "boom");
      return true;
    },
  );
});

test("falls back to statusText when text() rejects", async () => {
  globalThis.document = { cookie: "" };
  globalThis.fetch = async () =>
    okRes({
      ok: false,
      statusText: "Not Found",
      text: async () => {
        throw new Error("no body");
      },
    });

  await assert.rejects(
    () => apiFetch("/api/x"),
    (err) => {
      assert.equal(err.message, "Not Found");
      return true;
    },
  );
});

test("returns the response as-is when ok", async () => {
  globalThis.document = { cookie: "" };
  var res = okRes();
  globalThis.fetch = async () => res;

  var out = await apiFetch("/api/x");
  assert.equal(out, res);
});
