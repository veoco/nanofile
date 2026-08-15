// file-browser entry — loaded only on the file browser page (browser.html),
// after the common bundle. Bundles the browser layer; each module registers its
// own DOM listeners (event delegation, no window globals).
import "../browser/view.js";
import "../browser/list.js";
import "../browser/selection.js";
import "../browser/right-panel.js";
import "../browser/operations.js";
import "../browser/upload.js";
import "../browser/upload-link-dialog.js";
