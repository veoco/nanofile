// sysadmin-settings — the client-side behaviours of the settings page.
//
// Restarting cuts active transfers and clients reconnect, so the button asks first; the restart
// response then polls `/health` (same-origin, so `connect-src 'self'` allows it) until the
// replacement server answers. The filter box narrows rows by key, name or description. Waiting and
// matching are exported separately from the DOM wiring so bare Node can test them.
import { __t } from "../core/i18n.js";
import { ConfirmDialog } from "../core/confirm.js";

/** How long one `/health` probe may take before it counts as a failure. */
const PROBE_TIMEOUT_MS = 2000;
/** Gap between probes, and how long we wait before giving up. */
const POLL_INTERVAL_MS = 750;
const POLL_TIMEOUT_MS = 90_000;

/** Must match `server::restart::GENERATION_HEADER`. */
export const GENERATION_HEADER = "x-nanofile-generation";

/**
 * One readiness probe. Returns the generation the answering server reports alongside `ok`; any
 * failure is "not yet". A missing generation header (a stripping proxy) is `null`, and the caller
 * falls back to status alone.
 */
export async function probeHealth(fetchImpl) {
  const doFetch = fetchImpl || fetch;
  const controller = new AbortController();
  const timer = setTimeout(function () {
    controller.abort();
  }, PROBE_TIMEOUT_MS);
  try {
    const response = await doFetch("/health", {
      cache: "no-store",
      credentials: "same-origin",
      signal: controller.signal,
    });
    return {
      ok: response.ok,
      generation: response.headers ? response.headers.get(GENERATION_HEADER) : null,
    };
  } catch (e) {
    return { ok: false, generation: null };
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Whether a probe means "the replacement server is up". A 200 alone is not enough: the listening
 * socket survives a restart, so the generation must have changed. Without a generation to compare,
 * status alone decides — a moment early, but never a hang.
 */
export function isBack(probe, expectedGeneration) {
  if (!probe.ok) return false;
  if (!expectedGeneration) return true;
  if (probe.generation === null || probe.generation === undefined) return true;
  return String(probe.generation) !== String(expectedGeneration);
}

/**
 * Poll until `probe` resolves true, or the attempt budget runs out. Counted in attempts, not
 * wall-clock, so a slow probe cannot make the loop unbounded and a test needs no clock.
 */
export async function waitForServer(opts) {
  const probe = opts.probe;
  const sleep = opts.sleep;
  const attempts = opts.attempts || Math.ceil(POLL_TIMEOUT_MS / POLL_INTERVAL_MS);
  const intervalMs = opts.intervalMs || POLL_INTERVAL_MS;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    if (await probe()) return true;
    if (attempt + 1 < attempts) await sleep(intervalMs);
  }
  return false;
}

/**
 * Wire the restart-in-progress page, if this page is one. Returns whether a watch started, which
 * tells a caller (and a test) "settings form" from "server coming back".
 */
export function startRestartWatch(deps) {
  const options = deps || {};
  const doc = options.doc || document;
  const el = doc.querySelector("[data-restart-watch]");
  if (!el) return false;

  const location = options.location || window.location;
  const returnUrl = el.dataset.return || "/sysadmin/settings/";
  const setTimeoutImpl = options.setTimeout || setTimeout;
  const sleep =
    options.sleep ||
    function (ms) {
      return new Promise(function (resolve) {
        setTimeoutImpl(resolve, ms);
      });
    };
  const status = el.querySelector(".js-restart-status");
  const expectedGeneration = el.dataset.generation || "";
  const probe =
    options.probe ||
    function () {
      return probeHealth(options.fetchImpl);
    };

  waitForServer({
    probe: async function () {
      return isBack(await probe(), expectedGeneration);
    },
    sleep: sleep,
    attempts: options.attempts,
    intervalMs: options.intervalMs,
  }).then(function (healthy) {
    if (healthy) {
      location.replace(returnUrl);
      return;
    }
    // Out of patience: the manual link is already on the page, so only say why
    // nothing happened by itself.
    if (status && el.dataset.timeout) status.textContent = el.dataset.timeout;
  });

  return true;
}

/**
 * Ask before restarting. The button has a form of its own in the page header — restarting is not a
 * save — and the confirmation runs on the click; only a confirmed restart submits that form.
 */
export function initRestartConfirm(doc) {
  const document_ = doc || document;
  document_.addEventListener("click", function (event) {
    const button = event.target.closest("[data-restart-button]");
    if (!button) return;
    if (button.dataset.restartConfirmed === "1") return;
    event.preventDefault();
    ConfirmDialog.confirm(
      __t("setting.restart_button"),
      button.dataset.confirm || "",
      { confirmText: __t("setting.restart_button"), variant: "danger" },
    ).then(function (confirmed) {
      if (!confirmed) return;
      button.dataset.restartConfirmed = "1";
      const form = button.form;
      if (form && form.requestSubmit) {
        form.requestSubmit(button);
      } else {
        button.click();
      }
    });
  });
}

/**
 * Whether one row's searchable text matches a query: a case-insensitive substring test against
 * `data-setting-search` (key, name and description). A half-typed word must match, so ranking,
 * word boundaries and fuzzy distance would all make a partial query find nothing.
 */
export function matchesSetting(searchText, query) {
  const needle = String(query == null ? "" : query)
    .trim()
    .toLowerCase();
  if (!needle) return true;
  return String(searchText == null ? "" : searchText)
    .toLowerCase()
    .includes(needle);
}

/**
 * Wire the filter box, if this page has one. A row and its heading hide together: a heading with
 * nothing under it is worse than none, and its count follows what is left. `display: none` fields
 * still submit, so a filtered page saves what it was rendered with. Returns whether a box exists.
 */
export function initSettingsFilter(deps) {
  const options = deps || {};
  const doc = options.doc || document;
  const root = options.root || doc;
  const input = root.querySelector("[data-settings-filter]");
  if (!input) return false;

  const empty = root.querySelector("[data-settings-empty]");
  const groups = Array.from(root.querySelectorAll("[data-setting-group]"));

  // The page-wide count, and the text the server rendered into it. Counting the
  // rows here rather than trusting an attribute keeps this working on a page
  // whose markup is assembled by a test.
  const counter = root.querySelector("[data-settings-count]");
  const totalText = counter ? counter.textContent : "";
  let total = 0;
  groups.forEach(function (group) {
    total += group.querySelectorAll("[data-setting]").length;
  });

  function apply() {
    const query = input.value;
    let shown = 0;
    groups.forEach(function (group) {
      let inGroup = 0;
      group.querySelectorAll("[data-setting]").forEach(function (row) {
        const hit = matchesSetting(row.dataset.settingSearch, query);
        row.hidden = !hit;
        if (hit) inGroup += 1;
      });
      group.hidden = inGroup === 0;
      const count = group.querySelector("[data-setting-group-count]");
      if (count) count.textContent = String(inGroup);
      shown += inGroup;
    });
    if (empty) empty.hidden = shown > 0;
    // With nothing typed the page says what it holds; a query turns the same
    // line into "3 of 47", so the reader knows how much of the page is left.
    if (counter) {
      counter.textContent = query.trim()
        ? __t("setting.filter_count", { shown: String(shown), total: String(total) })
        : totalText;
    }
  }

  input.addEventListener("input", apply);
  apply();
  return true;
}

if (typeof document !== "undefined") {
  startRestartWatch();
  initRestartConfirm();
  initSettingsFilter();
}
