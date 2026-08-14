// Nanofile Web UI — common.js
// Shared browser utilities, loaded before main.js / file-browser.js so both
// the bundled scripts and any template inline <script> blocks can use them.
(function () {
  "use strict";

  // HTML-escape for text content. Escapes `& < >` only, so it is safe to
  // interpolate into text nodes / innerHTML text positions — NOT attributes.
  function escapeHtml(str) {
    var div = document.createElement("div");
    div.appendChild(document.createTextNode(str == null ? "" : String(str)));
    return div.innerHTML;
  }

  // HTML-escape for attribute values. Also escapes single/double quotes, so it
  // is safe to interpolate into double-quoted HTML attributes.
  function escapeAttr(str) {
    return String(str == null ? "" : str)
      .replace(/&/g, "&amp;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function getCookie(name) {
    var match = document.cookie.match("(^|;)\\s*" + name + "\\s*=\\s*([^;]+)");
    return match ? match.pop() : "";
  }

  // Encode a "/"-separated file path so each segment is URI-encoded while
  // slashes are preserved. Used for /repos/{id}/files/{path} URLs.
  function encodeFilePath(path) {
    return String(path).split("/").map(encodeURIComponent).join("/");
  }

  // Strip surrounding double quotes from a value: "foo" → foo.
  function unquote(v) {
    return String(v).replace(/^"|"$/g, "");
  }

  // Validate a CSS color before interpolating it into a style attribute so a
  // user-supplied tag color cannot inject CSS. Only hex is accepted.
  function safeColor(color, fallback) {
    var c = String(color || "");
    if (/^#[0-9a-fA-F]{3,8}$/.test(c)) return c;
    return fallback || "#e6e6e6";
  }

  window.escapeHtml = escapeHtml;
  window.escapeAttr = escapeAttr;
  window.getCookie = getCookie;
  window.encodeFilePath = encodeFilePath;
  window.unquote = unquote;
  window.safeColor = safeColor;
})();
