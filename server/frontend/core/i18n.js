// i18n — translation helper. Strings are injected into window.__T by the
// server (see base.html). Falls back to the key itself so untranslated strings
// are easy to spot.
export function __t(key, args) {
  var s = (window.__T && window.__T[key]) || key;
  if (args) {
    for (var k in args) {
      s = s.split("{" + k + "}").join(String(args[k]));
    }
  }
  return s;
}
