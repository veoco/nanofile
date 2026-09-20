import { test } from "node:test";
import assert from "node:assert/strict";

// ── Minimal DOM, just enough for local-time's day grouping ──────────────────
//
// The frontend tests run on bare Node (no jsdom dependency), and the grouping
// only needs element creation, `classList`, `dataset`, sibling links and tree
// queries — so this shim stands in for the browser rather than adding a dep.
function datasetKey(attr) {
  return attr.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
}

class FakeElement {
  constructor(tag) {
    this.tagName = String(tag).toUpperCase();
    this.children = [];
    this.parentNode = null;
    this.dataset = {};
    this._text = "";
    this._classes = new Set();
    this._attrs = new Set();
    this.classList = {
      contains: (name) => this._classes.has(name),
      add: (name) => this._classes.add(name),
    };
  }
  get className() {
    return [...this._classes].join(" ");
  }
  set className(value) {
    this._classes = new Set(String(value).split(/\s+/).filter(Boolean));
  }
  get textContent() {
    return this._text + this.children.map((c) => c.textContent).join("");
  }
  set textContent(value) {
    this._text = String(value);
    this.children = [];
  }
  get previousElementSibling() {
    if (!this.parentNode) return null;
    const i = this.parentNode.children.indexOf(this);
    return i > 0 ? this.parentNode.children[i - 1] : null;
  }
  hasAttribute(name) {
    return name.startsWith("data-") ? datasetKey(name) in this.dataset : this._attrs.has(name);
  }
  setAttribute(name) {
    this._attrs.add(name);
  }
  appendChild(child) {
    child.parentNode = this;
    this.children.push(child);
    return child;
  }
  insertBefore(node, ref) {
    node.parentNode = this;
    const i = this.children.indexOf(ref);
    if (i < 0) this.children.push(node);
    else this.children.splice(i, 0, node);
    return node;
  }
  remove() {
    const parent = this.parentNode;
    if (!parent) return;
    const i = parent.children.indexOf(this);
    if (i >= 0) parent.children.splice(i, 1);
    this.parentNode = null;
  }
  matches(selector) {
    if (selector === "[data-ts-day]") return "tsDay" in this.dataset;
    if (selector === "[data-ts]") return "ts" in this.dataset;
    return false;
  }
  querySelectorAll(selector) {
    const found = [];
    for (const child of this.children) {
      if (child.matches(selector)) found.push(child);
      found.push(...child.querySelectorAll(selector));
    }
    return found;
  }
}

globalThis.window = {
  __T: { "activity.today": "Today", "activity.yesterday": "Yesterday" },
};
// `lang` is what core/format.js hands `Intl`; pinning it keeps the day-header
// assertion below from following the machine's default locale.
globalThis.document = {
  createElement: (tag) => new FakeElement(tag),
  documentElement: { lang: "en" },
};

const { renderAll } = await import("./local-time.js");

// Local-midnight-safe seconds for a calendar date, so the shim and the module
// agree on the reader's day regardless of the machine's timezone.
function localTs(year, month, day, hour) {
  return Math.floor(new Date(year, month - 1, day, hour).getTime() / 1000);
}

function activityList(timestamps) {
  const list = new FakeElement("div");
  for (const ts of timestamps) {
    const row = new FakeElement("div");
    row.className = "nf-prow";
    row.dataset.tsDay = String(ts);
    list.appendChild(row);
  }
  return list;
}

// "row", or "H:<label>" for a day header, in document order.
function layout(list) {
  return list.children.map((el) =>
    el.classList.contains("nf-sec") ? `H:${el.textContent.trim()}` : "row",
  );
}

test("a day header is emitted once per local calendar day", () => {
  const list = activityList([
    localTs(2020, 3, 5, 12),
    localTs(2020, 3, 5, 9),
    localTs(2020, 3, 5, 8),
    localTs(2020, 3, 4, 23),
    localTs(2020, 3, 4, 1),
    localTs(2020, 3, 2, 7),
  ]);

  renderAll(list);

  assert.deepEqual(layout(list), [
    "H:Mar 5, 2020",
    "row",
    "row",
    "row",
    "H:Mar 4, 2020",
    "row",
    "row",
    "H:Mar 2, 2020",
    "row",
  ]);
});

test("re-rendering the same list does not stack duplicate headers", () => {
  const list = activityList([
    localTs(2020, 3, 5, 12),
    localTs(2020, 3, 5, 9),
    localTs(2020, 3, 4, 1),
  ]);

  renderAll(list);
  const once = layout(list);
  renderAll(list);

  assert.deepEqual(layout(list), once);
});

test("a stray header between two rows of one day is removed", () => {
  const list = activityList([localTs(2020, 3, 5, 12), localTs(2020, 3, 5, 9)]);
  renderAll(list);

  const stray = new FakeElement("div");
  stray.className = "nf-sec";
  stray.textContent = "Mar 5, 2020";
  const secondRow = list.children[2];
  list.insertBefore(stray, secondRow);

  renderAll(list);

  assert.deepEqual(layout(list), ["H:Mar 5, 2020", "row", "row"]);
});

test("the current local day is labelled Today", () => {
  // Local noon, so both rows stay on one side of midnight however the test is
  // run — the point is the label, not the ordering of `now`.
  const now = new Date();
  const noon = Math.floor(new Date(now.getFullYear(), now.getMonth(), now.getDate(), 12).getTime() / 1000);
  const list = activityList([noon, noon - 3600]);

  renderAll(list);

  assert.deepEqual(layout(list), ["H:Today", "row", "row"]);
});

test("an empty list gets no header", () => {
  const list = activityList([]);
  renderAll(list);
  assert.deepEqual(layout(list), []);
});
