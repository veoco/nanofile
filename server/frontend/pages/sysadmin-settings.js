// sysadmin-settings — the two client-side behaviours of the settings page.
//
//   1. Restarting is not idempotent for the people using the server (active
//      transfers are cut, clients reconnect), so the button asks first.
//   2. The restart response is a page that says "restarting" and then waits:
//      the server is down for a moment, and the browser has to find out when
//      it is back before reloading. `/health` is the readiness probe the
//      container healthcheck already uses, and it is same-origin, which is what
//      `connect-src 'self'` allows.
//
// The waiting logic is exported separately from the DOM wiring so it can be
// tested on bare Node (see `sysadmin-settings.test.js`).
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
 * One readiness probe.
 *
 * Returns the generation the answering server reports alongside `ok`. Any
 * failure — refused connection, timeout, a 503 from a draining server — is
 * simply "not yet". The generation header may be missing (a proxy that strips
 * it), in which case it is `null` and the caller falls back to status alone.
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
 * Whether a probe means "the replacement server is up".
 *
 * A 200 alone is not enough: the listening socket survives the restart, so a
 * request sent while the old server is still shutting down is answered by it.
 * The generation has to have *changed* for the server to count as back. When
 * the page has no generation to compare (or the header was stripped) the status
 * alone decides, which can reload a moment early but never hangs.
 */
export function isBack(probe, expectedGeneration) {
  if (!probe.ok) return false;
  if (!expectedGeneration) return true;
  if (probe.generation === null || probe.generation === undefined) return true;
  return String(probe.generation) !== String(expectedGeneration);
}

/**
 * Poll until `probe` resolves true, or the attempt budget runs out.
 *
 * Counted in attempts rather than wall-clock time so a slow probe cannot make
 * the loop unbounded, and so the caller can test it without a clock.
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
 * Wire the restart-in-progress page, if this page is one.
 *
 * Returns whether a watch was started, which gives the caller (and a test) a
 * way to tell "this is the settings form" from "the server is coming back".
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
 * Ask before restarting.
 *
 * A `formaction` submit button keeps the restart inside the settings form (a
 * nested form would be invalid HTML), so the interception is on the click: the
 * confirmation runs first, and only then is the button's own action submitted.
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

if (typeof document !== "undefined") {
  startRestartWatch();
  initRestartConfirm();
}
