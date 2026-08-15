// modal — unified backdrop-close handling for the shared modal macro
// (includes/modal.html). The macro emits `data-backdrop-close="<name>"` on the
// overlay and `data-modal-card` on the inner card; clicking the backdrop
// (but not the card) closes the overlay.

var backdropHandlers = {};

/** Register a named backdrop-close handler (called instead of the default hide). */
export function registerModalClose(name, fn) {
  backdropHandlers[name] = fn;
}

document.addEventListener("click", function (e) {
  var overlay = e.target.closest("[data-backdrop-close]");
  if (!overlay || e.target !== overlay) return;
  var name = overlay.dataset.backdropClose;
  var fn = backdropHandlers[name];
  if (fn) fn();
  else overlay.classList.add("hidden");
});
