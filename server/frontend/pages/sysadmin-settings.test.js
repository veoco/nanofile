import { test } from "node:test";
import assert from "node:assert/strict";

import {
  probeHealth,
  isBack,
  waitForServer,
  startRestartWatch,
  matchesSetting,
  initSettingsFilter,
} from "./sysadmin-settings.js";

/** Let the promise chain inside `startRestartWatch` settle. */
function flush() {
  return new Promise(function (resolve) {
    setImmediate(resolve);
  });
}

test("waitForServer retries until the probe succeeds", async () => {
  let calls = 0;
  let slept = 0;
  const healthy = await waitForServer({
    probe: async () => {
      calls += 1;
      return calls >= 3;
    },
    sleep: async () => {
      slept += 1;
    },
  });
  assert.equal(healthy, true);
  assert.equal(calls, 3);
  // No pointless sleep after the successful probe.
  assert.equal(slept, 2);
});

test("waitForServer gives up when the budget runs out", async () => {
  let calls = 0;
  const healthy = await waitForServer({
    attempts: 3,
    probe: async () => {
      calls += 1;
      return false;
    },
    sleep: async () => {},
  });
  assert.equal(healthy, false);
  assert.equal(calls, 3);
});

test("probeHealth treats a refused connection as 'not yet'", async () => {
  const rejected = await probeHealth(async () => {
    throw new Error("connection refused");
  });
  assert.equal(rejected.ok, false);

  const draining = await probeHealth(async () => ({ ok: false }));
  assert.equal(draining.ok, false);

  const up = await probeHealth(async () => ({
    ok: true,
    headers: { get: () => "7" },
  }));
  assert.equal(up.ok, true);
  assert.equal(up.generation, "7");
});

test("probeHealth asks for the readiness endpoint uncached", async () => {
  let seen = null;
  await probeHealth(async (url, init) => {
    seen = { url, init };
    return { ok: true, headers: { get: () => null } };
  });
  assert.equal(seen.url, "/health");
  assert.equal(seen.init.cache, "no-store");
});

test("isBack needs a *different* generation, not just a 200", () => {
  // The old server answers /health while it shuts down, because the listening
  // socket survives the restart. Only a changed generation means "back".
  assert.equal(isBack({ ok: true, generation: "3" }, "3"), false);
  assert.equal(isBack({ ok: true, generation: "4" }, "3"), true);
  assert.equal(isBack({ ok: false, generation: "4" }, "3"), false);
  // No generation to compare (no marker, or a proxy stripped the header): fall
  // back to the status rather than waiting forever.
  assert.equal(isBack({ ok: true, generation: "3" }, ""), true);
  assert.equal(isBack({ ok: true, generation: null }, "3"), true);
});

test("startRestartWatch reloads once the server answers", async () => {
  const status = { textContent: "" };
  const el = {
    dataset: {
      return: "/sysadmin/settings/server/?action=restarted",
      timeout: "gave up",
      generation: "1",
    },
    querySelector: () => status,
  };
  const location = {
    replaced: null,
    replace: function (url) {
      this.replaced = url;
    },
  };

  const started = startRestartWatch({
    doc: { querySelector: (sel) => (sel === "[data-restart-watch]" ? el : null) },
    probe: async () => ({ ok: true, generation: "2" }),
    sleep: async () => {},
    location: location,
  });
  assert.equal(started, true);

  await flush();
  assert.equal(location.replaced, "/sysadmin/settings/server/?action=restarted");
  assert.equal(status.textContent, "", "a successful wait needs no message");
});

test("startRestartWatch explains a timeout and does not navigate", async () => {
  const status = { textContent: "" };
  const el = {
    dataset: {
      return: "/sysadmin/settings/",
      timeout: "the server did not come back",
      generation: "1",
    },
    querySelector: () => status,
  };
  const location = {
    replaced: null,
    replace: function (url) {
      this.replaced = url;
    },
  };

  startRestartWatch({
    doc: { querySelector: () => el },
    probe: async () => ({ ok: true, generation: "1" }),
    sleep: async () => {},
    attempts: 2,
    location: location,
  });

  await flush();
  assert.equal(location.replaced, null);
  assert.equal(status.textContent, "the server did not come back");
});

test("startRestartWatch is inert on a page without the marker", () => {
  assert.equal(startRestartWatch({ doc: { querySelector: () => null } }), false);
});

// ─── Filter ───────────────────────────────────────────────────────────────

test("matchesSetting keeps every row when nothing is typed", () => {
  assert.equal(matchesSetting("server.port Bind port", ""), true);
  assert.equal(matchesSetting("server.port Bind port", "   "), true);
  assert.equal(matchesSetting("", ""), true);
});

test("matchesSetting is case-insensitive and matches anywhere", () => {
  const row = "server.max_upload_size_mb Max upload size The transport cap";
  assert.equal(matchesSetting(row, "UPLOAD"), true);
  assert.equal(matchesSetting(row, "max_upload"), true);
  assert.equal(matchesSetting(row, "transport"), true);
  assert.equal(matchesSetting(row, "download"), false);
});

test("matchesSetting tolerates a missing row text", () => {
  assert.equal(matchesSetting(undefined, "x"), false);
  assert.equal(matchesSetting(null, "x"), false);
});

/**
 * A minimal settings page: the filter box, two groups, and rows whose
 * `dataset.settingSearch` is what the server rendered into the attribute.
 */
function fakeSettingsPage() {
  function makeRow(search) {
    return { dataset: { settingSearch: search }, hidden: false };
  }
  const rows = {
    addr: makeRow("server.addr Bind address Changing the listener needs a restart."),
    port: makeRow("server.port Bind port Changing the listener needs a restart."),
    host: makeRow("email.host SMTP host Host name of the SMTP server."),
  };
  function makeGroup(held) {
    const count = { textContent: String(held.length) };
    return {
      hidden: false,
      count,
      querySelectorAll: (sel) => (sel === "[data-setting]" ? held : []),
      querySelector: (sel) => (sel === "[data-setting-group-count]" ? count : null),
    };
  }
  const groups = [makeGroup([rows.addr, rows.port]), makeGroup([rows.host])];
  const input = {
    value: "",
    listeners: {},
    addEventListener: function (event, fn) {
      this.listeners[event] = fn;
    },
  };
  const empty = { hidden: true };
  return {
    input,
    empty,
    groups,
    rows,
    doc: {
      querySelector: (sel) => {
        if (sel === "[data-settings-filter]") return input;
        if (sel === "[data-settings-empty]") return empty;
        return null;
      },
      querySelectorAll: (sel) => (sel === "[data-setting-group]" ? groups : []),
    },
    type: function (value) {
      this.input.value = value;
      this.input.listeners.input();
    },
  };
}

test("initSettingsFilter hides rows and a heading left with none", () => {
  const page = fakeSettingsPage();
  assert.equal(initSettingsFilter({ doc: page.doc }), true);

  // Nothing typed: every row and heading is visible, and the note is hidden.
  assert.equal(page.empty.hidden, true);
  assert.equal(page.groups[0].hidden, false);

  page.type("email");
  assert.equal(page.rows.addr.hidden, true);
  assert.equal(page.rows.host.hidden, false);
  assert.equal(page.groups[0].hidden, true, "a heading with no rows goes with them");
  assert.equal(page.groups[1].hidden, false);
  assert.equal(page.empty.hidden, true);
  // A heading counts what is left under it, not what it started with.
  assert.equal(page.groups[1].count.textContent, "1");

  // A query that matches nothing says so rather than showing a blank page.
  page.type("nothing matches this");
  assert.equal(page.groups[1].hidden, true);
  assert.equal(page.empty.hidden, false);

  // Clearing the box brings the page back.
  page.type("");
  assert.equal(page.groups[0].hidden, false);
  assert.equal(page.groups[1].hidden, false);
  assert.equal(page.groups[0].count.textContent, "2");
  assert.equal(page.empty.hidden, true);
});

test("initSettingsFilter is inert on a page without a box", () => {
  assert.equal(
    initSettingsFilter({ doc: { querySelector: () => null, querySelectorAll: () => [] } }),
    false,
  );
});
