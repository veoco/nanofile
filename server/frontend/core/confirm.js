// confirm — custom confirm dialog (replaces native confirm()).
import { __t } from "./i18n.js";
import { escapeHtml } from "./utils.js";

var confirmOverlay = null;
var confirmResolve = null;

function initConfirmDialog() {
  confirmOverlay = document.createElement("div");
  confirmOverlay.className =
    "hidden fixed inset-0 z-above-dialog flex items-center justify-center bg-black/30";
  confirmOverlay.setAttribute("role", "alertdialog");
  confirmOverlay.setAttribute("aria-modal", "true");
  confirmOverlay.setAttribute("aria-labelledby", "nf-confirm-title");
  confirmOverlay.setAttribute("aria-describedby", "nf-confirm-message");
  confirmOverlay.innerHTML =
    '<div class="bg-panel border border-line rounded-box p-5 w-full max-w-sm mx-4">' +
    '<h3 id="nf-confirm-title" class="text-[15px] font-semibold text-ink mb-1 js-confirm-title"></h3>' +
    '<p id="nf-confirm-message" class="text-[13px] text-ink-2 mb-4 js-confirm-message"></p>' +
    '<div class="flex justify-end gap-2">' +
    '<button type="button" class="js-confirm-cancel btn btn-line">' + escapeHtml(__t("common.cancel")) + "</button>" +
    '<button type="button" class="js-confirm-ok btn"></button>' +
    "</div></div>";
  document.body.appendChild(confirmOverlay);

  confirmOverlay.addEventListener("click", function (e) {
    if (e.target === confirmOverlay) hideConfirm(false);
  });

  confirmOverlay.querySelector(".js-confirm-cancel").addEventListener("click", function () {
    hideConfirm(false);
  });

  document.addEventListener("keydown", function confirmEsc(e) {
    if (e.key === "Escape" && confirmOverlay && !confirmOverlay.classList.contains("hidden")) {
      hideConfirm(false);
    }
  });
}

function hideConfirm(result) {
  if (confirmOverlay) confirmOverlay.classList.add("hidden");
  if (confirmResolve) { confirmResolve(result); confirmResolve = null; }
}

function showConfirmDialog(title, message, opts) {
  opts = opts || {};
  if (!confirmOverlay) initConfirmDialog();

  confirmOverlay.querySelector(".js-confirm-title").textContent = title;
  confirmOverlay.querySelector(".js-confirm-message").textContent = message;

  var okBtn = confirmOverlay.querySelector(".js-confirm-ok");
  okBtn.textContent = opts.confirmText || __t("ui.delete");
  // The variant decides which design-system button the action wears: the
  // destructive one is `.btn-danger`, everything else `.btn-solid`. Both pair
  // their own fill with their own ink, so they stay readable in either theme.
  okBtn.className =
    "js-confirm-ok btn " + (opts.variant === "danger" ? "btn-danger" : "btn-solid");

  // Remove old listener by cloning
  var newOk = okBtn.cloneNode(true);
  okBtn.parentNode.replaceChild(newOk, okBtn);

  confirmOverlay.classList.remove("hidden");
  // Focus the cancel button by default
  setTimeout(function () {
    confirmOverlay.querySelector(".js-confirm-cancel").focus();
  }, 100);

  return new Promise(function (resolve) {
    confirmResolve = resolve;
    newOk.addEventListener("click", function () { hideConfirm(true); });
  });
}

export const ConfirmDialog = {
  confirm: function (title, message, opts) { return showConfirmDialog(title, message, opts); },
};
