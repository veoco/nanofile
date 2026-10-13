// preview-failure — what a preview says when it cannot show the file.
//
// The full-page viewer and the double-click dialog are two ways to look at the
// same bytes, so a file the browser cannot decode has to read the same in both:
// the reason the render failed, and the same escape — download the original.
// Both surfaces render the node built here, so the wording and the escape
// cannot drift apart again.
import { __t } from "./i18n.js";

// Why a preview failed, as the locale key each surface shows. `image` is a
// decode failure, `media` is a container or a codec the browser does not have,
// `text` is the dialog's fetch failing (the full page renders text on the
// server, so it has no such case). Asserted against `locales/*.toml` by
// preview-failure.test.js.
export var PREVIEW_FAILURE = {
  image: "fb.image_failed_to_load",
  media: "fb.media_unsupported",
  text: "fb.preview_failed",
};

// The failure node for `kind`: the reason, then `fb.download_instead` when
// there is a URL to point it at. Built with DOM APIs (not `innerHTML`) so the
// `?dl=1` URL stays inert, like the picture fallback it replaces.
export function previewFailureNode(kind, downloadUrl) {
  var wrap = document.createElement("div");
  wrap.className = "nf-preview-fallback";

  var message = document.createElement("p");
  message.textContent = __t(PREVIEW_FAILURE[kind] || PREVIEW_FAILURE.text);
  wrap.appendChild(message);

  if (downloadUrl) {
    var link = document.createElement("a");
    link.href = downloadUrl;
    link.textContent = __t("fb.download_instead");
    wrap.appendChild(link);
  }
  return wrap;
}

// Replace whatever `container` holds with that node. Both surfaces call this:
// the viewer's stage holds the one element that failed, the dialog's message
// box holds the last failure.
export function showPreviewFailure(container, kind, downloadUrl) {
  if (!container) return;
  while (container.firstChild) container.removeChild(container.firstChild);
  container.appendChild(previewFailureNode(kind, downloadUrl));
}
