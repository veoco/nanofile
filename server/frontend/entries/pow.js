// Browser entry for the anonymous form challenge. The server renders one
// `[data-pow-challenge]` element per protected form; without it this file does
// nothing, so it is safe to include on any auth page.
import { hexToBytes, solvePow } from "../core/pow.js";

function setStatus(el, text) {
  var status = el.querySelector("[data-pow-status]");
  if (status && typeof text === "string") status.textContent = text;
}

function initChallenge(el) {
  var form = el.closest("form");
  if (!form) return;
  var nonceInput = form.querySelector('input[name="pow_nonce"]');
  if (!nonceInput) return;
  var button = form.querySelector('button[type="submit"], input[type="submit"]');
  var bits = parseInt(el.getAttribute("data-pow-bits") || "", 10);
  var seed;
  try {
    seed = hexToBytes(el.getAttribute("data-pow-seed") || "");
  } catch (e) {
    seed = null;
  }

  // Never let a form with an unsolved challenge reach the server.
  form.addEventListener("submit", function (event) {
    if (!nonceInput.value) event.preventDefault();
  });

  if (!seed || seed.length !== 16 || !(bits >= 1 && bits <= 32)) {
    setStatus(el, el.dataset.powError || "");
    return;
  }

  if (button) button.disabled = true;
  setStatus(el, el.dataset.powWorking || "");

  solvePow(seed, bits)
    .then(function (nonce) {
      if (nonce === null) {
        setStatus(el, el.dataset.powError || "");
        if (button) button.disabled = false;
        return;
      }
      nonceInput.value = String(nonce);
      setStatus(el, el.dataset.powReady || "");
      if (button) button.disabled = false;
    })
    .catch(function () {
      setStatus(el, el.dataset.powError || "");
      if (button) button.disabled = false;
    });
}

function init() {
  var challenges = document.querySelectorAll("[data-pow-challenge]");
  for (var i = 0; i < challenges.length; i += 1) initChallenge(challenges[i]);
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", init);
} else {
  init();
}
