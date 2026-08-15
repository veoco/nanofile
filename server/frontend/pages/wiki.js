// wiki — knowledge base list + view page interactions (modal open/close and
// confirmation). Mutation forms submit natively to server-rendered POST
// routes; this module only drives modal visibility, pre-fills fields, and
// confirms destructive actions.

import { __t } from "../core/i18n.js";
import { registerModalClose } from "../core/modal.js";

function show(id) {
  var el = document.getElementById(id);
  if (el) el.classList.remove("hidden");
}

function hide(id) {
  var el = document.getElementById(id);
  if (el) el.classList.add("hidden");
}

function focus(id, select) {
  setTimeout(function () {
    var el = document.getElementById(id);
    if (!el) return;
    el.focus();
    if (select) el.select();
  }, 100);
}

// ─── Wiki list page ──────────────────────────────────────────────────────

function showCreate() {
  var input = document.getElementById("wiki-create-input");
  if (!input) return;
  input.value = "";
  show("wiki-create-overlay");
  focus("wiki-create-input");
}

function showRename(btn) {
  var form = document.getElementById("wiki-rename-form");
  var input = document.getElementById("wiki-rename-input");
  if (!form || !input) return;
  form.action = "/wikis/" + btn.dataset.id + "/rename/";
  input.value = btn.dataset.name || "";
  show("wiki-rename-overlay");
  focus("wiki-rename-input", true);
}

function showPublish(btn) {
  var form = document.getElementById("wiki-publish-form");
  var input = document.getElementById("wiki-publish-url");
  if (!form || !input) return;
  form.action = "/wikis/" + btn.dataset.id + "/publish/";
  input.value = "";
  show("wiki-publish-overlay");
  focus("wiki-publish-url");
}

// ─── Wiki view page ──────────────────────────────────────────────────────

function showPageCreate(btn) {
  var currentId = document.getElementById("page-create-current-id");
  var position = document.getElementById("page-create-insert-position");
  var input = document.getElementById("page-create-input");
  if (currentId) currentId.value = btn.dataset.currentId || "";
  if (position) position.value = btn.dataset.insertPosition || "";
  if (input) input.value = "";
  show("page-create-overlay");
  focus("page-create-input");
}

function showPageRename(btn) {
  var input = document.getElementById("page-rename-input");
  if (input) input.value = btn.dataset.name || "";
  show("page-rename-overlay");
  focus("page-rename-input", true);
}

function showPageMove() {
  show("page-move-overlay");
}

document.addEventListener("click", function (e) {
  var el = e.target.closest("[data-action]");
  if (!el) return;
  switch (el.dataset.action) {
    case "show-wiki-create": showCreate(); break;
    case "show-wiki-rename": showRename(el); break;
    case "show-wiki-publish": showPublish(el); break;
    case "close-wiki-create": hide("wiki-create-overlay"); break;
    case "close-wiki-rename": hide("wiki-rename-overlay"); break;
    case "close-wiki-publish": hide("wiki-publish-overlay"); break;
    case "show-page-create": showPageCreate(el); break;
    case "show-page-rename": showPageRename(el); break;
    case "show-page-move": showPageMove(); break;
    case "close-page-create": hide("page-create-overlay"); break;
    case "close-page-rename": hide("page-rename-overlay"); break;
    case "close-page-move": hide("page-move-overlay"); break;
  }
});

// Confirm before deleting a wiki or a page (server-rendered POST form).
document.addEventListener("submit", function (e) {
  var form = e.target.closest("form[data-confirm-name], form[data-confirm-page-name]");
  if (!form) return;
  var name = form.dataset.confirmName || form.dataset.confirmPageName || "";
  var key = form.dataset.confirmName ? "wiki.delete_confirm" : "wiki.delete_page_confirm";
  if (!confirm(__t(key, { name: name }))) {
    e.preventDefault();
  }
});

registerModalClose("hideWikiCreate", function () { hide("wiki-create-overlay"); });
registerModalClose("hideWikiRename", function () { hide("wiki-rename-overlay"); });
registerModalClose("hideWikiPublish", function () { hide("wiki-publish-overlay"); });
registerModalClose("hidePageCreate", function () { hide("page-create-overlay"); });
registerModalClose("hidePageRename", function () { hide("page-rename-overlay"); });
registerModalClose("hidePageMove", function () { hide("page-move-overlay"); });
