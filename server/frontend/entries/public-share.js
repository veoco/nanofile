// Shared behaviour for the public share pages (/f/, /d/), which do not load
// the authenticated UI bundle.
//
// The pages render timestamps as `data-ts` (unix seconds) attributes so the
// browser formats them in the visitor's timezone; this used to be an inline
// <script> in each template, which forced the Content-Security-Policy to allow
// 'unsafe-inline' for scripts.
import { initLocalTime } from "../core/local-time.js";

function start() {
  initLocalTime();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start);
} else {
  start();
}
