// preview-failure — what a preview says when it cannot show the file. The full-page viewer and
// the dialog are two views of the same bytes, so both render the node built here and the reason
// and download escape cannot drift apart.
import { __t } from "./i18n.js";

// Why a preview failed, as the locale key each surface shows: `image` is a decode failure,
// `media` a container or codec the browser lacks, `text` the dialog's fetch. Asserted against
// `locales/*.toml` by preview-failure.test.js.
export var PREVIEW_FAILURE = {
  image: "fb.image_failed_to_load",
  media: "fb.media_unsupported",
  text: "fb.preview_failed",
};

// The failure node: the reason, then `fb.download_instead` when there is a URL. Built with DOM
// APIs so the `?dl=1` URL stays inert.
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

// Replace whatever `container` holds: the viewer's stage holds the element that failed, the
// dialog's box the last failure.
export function showPreviewFailure(container, kind, downloadUrl) {
  if (!container) return;
  while (container.firstChild) container.removeChild(container.firstChild);
  container.appendChild(previewFailureNode(kind, downloadUrl));
}
