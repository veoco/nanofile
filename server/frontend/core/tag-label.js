// tag-label — the display text for a tag. An automatic tag's stored name is
// only a snapshot of the language at scan time, but the tag also carries the
// stable `labelKey`; translate it again for the current viewer. Hand-made tags
// are user data and keep the name their owner gave them.
import { __t } from "./i18n.js";

export function tagLabel(tag) {
  if (!tag) return "";
  if (tag.labelKey) {
    var key = "fb.tag_" + tag.labelKey;
    var translated = __t(key);
    if (translated !== key) return translated;
  }
  return tag.name || "";
}
