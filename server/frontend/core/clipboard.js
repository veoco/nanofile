// clipboard — one guarded copy path and one feedback convention.
//
// `navigator.clipboard` only exists in a secure context; on a plain-HTTP LAN
// address it is either missing or rejects. The textarea fallback keeps the
// copy buttons working there and reports honestly whether the copy happened.

export function copyText(text) {
  var value = text == null ? "" : String(text);
  if (!value) return Promise.resolve(false);

  if (
    typeof navigator !== "undefined" &&
    navigator.clipboard &&
    typeof navigator.clipboard.writeText === "function"
  ) {
    return navigator.clipboard.writeText(value).then(
      function () {
        return true;
      },
      function () {
        return fallbackCopy(value);
      },
    );
  }
  return Promise.resolve(fallbackCopy(value));
}

function fallbackCopy(value) {
  if (typeof document === "undefined" || !document.createElement) return false;
  var area = document.createElement("textarea");
  area.value = value;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.top = "-1000px";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  var copied = false;
  try {
    copied = document.execCommand("copy");
  } catch (error) {
    copied = false;
  }
  document.body.removeChild(area);
  return copied;
}

/// Replace a button's label for a moment; repeated clicks restart the timer.
export function flashCopied(button, label, durationMs) {
  if (!button) return;
  if (button._copyTimer) clearTimeout(button._copyTimer);
  var original = button.textContent;
  button.textContent = label;
  button._copyTimer = setTimeout(function () {
    button.textContent = original;
    button._copyTimer = null;
  }, durationMs || 1500);
}
