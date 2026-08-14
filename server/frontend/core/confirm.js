// confirm — custom confirm dialog (replaces native confirm()).
import { __t } from "./i18n.js";

var confirmOverlay = null;
var confirmResolve = null;

function initConfirmDialog() {
  confirmOverlay = document.createElement("div");
  confirmOverlay.className =
    "hidden fixed inset-0 z-[100] flex items-center justify-center bg-black/30";
  confirmOverlay.setAttribute("role", "alertdialog");
  confirmOverlay.setAttribute("aria-modal", "true");
  confirmOverlay.innerHTML =
    '<div class="bg-white dark:bg-surface-800 rounded-xl shadow-xl p-6 w-full max-w-sm mx-4" onclick="event.stopPropagation()">' +
    '<h3 class="text-base font-semibold text-gray-900 dark:text-gray-100 mb-1 js-confirm-title"></h3>' +
    '<p class="text-sm text-gray-500 dark:text-gray-400 mb-4 js-confirm-message"></p>' +
    '<div class="flex justify-end gap-2">' +
    '<button class="js-confirm-cancel rounded-lg bg-white dark:bg-surface-700 px-4 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 border border-gray-300 dark:border-gray-600 hover:bg-gray-50 dark:hover:bg-surface-600 transition-colors">Cancel</button>' +
    '<button class="js-confirm-ok rounded-lg px-4 py-2 text-sm font-medium text-white transition-colors"></button>' +
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
  okBtn.textContent = opts.confirmText || __t('ui.delete');
  okBtn.className =
    "js-confirm-ok rounded-lg px-4 py-2 text-sm font-medium text-white transition-colors " +
    (opts.variant === "danger"
      ? "bg-red-600 hover:bg-red-700"
      : "bg-brand-500 hover:bg-brand-600");

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
