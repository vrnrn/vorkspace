import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { setImmediate } from "node:timers/promises";
import { test } from "node:test";
import vm from "node:vm";

const source = await readFile(new URL("../../crates/vorkspace-gpui/src/meetings/ui.html", import.meta.url), "utf8");
const script = source.match(/<script>([\s\S]*?)<\/script>/)[1];
// A small event/element adapter keeps scheduling deterministic. Real layout,
// opaque-origin isolation and native controls are exercised separately in WebKit.
class Element {
  constructor() {
    this.value = ""; this.textContent = ""; this.innerHTML = ""; this.style = {};
    this.children = []; this.attributes = {}; this.hidden = false;
    this.scrollHeight = 0; this.scrollTop = 0; this.clientHeight = 400;
    const classes = new Set();
    this.classList = { toggle: (name, on) => on ? classes.add(name) : classes.delete(name) };
  }
  setAttribute(name, value) { this.attributes[name] = value; }
  replaceChildren() { this.children = []; }
  append(...children) { this.children.push(...children); }
  querySelector() { return this.span ??= new Element(); }
}
function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}
function document(id) {
  return {
    meeting: { id, title: id, revision: 0, transcript_revision: 0, summary: { text: "Original", source_revision: 0 } },
    state: { id, started_at: 1000, ended_at: 2000, phase: "stopped" },
    transcript: `${id} transcript`, segments: [], sequence: 0, summary_html: "<p>Original</p>",
  };
}
async function harness() {
  const elements = new Map(), intervals = [], records = new Map([['A', document('A')], ['B', document('B')]]);
  const get = (id) => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); };
  const pending = new Map(), writes = [], stops = [];
  let phase = "starting";
  const call = async (method, params = {}) => {
    if (pending.has(method + ":" + (params.id ?? ""))) await pending.get(method + ":" + (params.id ?? "")).promise;
    if (method === "meetings.list") return [...records.values()].map(({ meeting, state }) => structuredClone({ meeting, state }));
    if (method === "meetings.read") return structuredClone(records.get(params.id));
    if (method === "meetings.start") return { id: "live" };
    if (method === "meetings.summarize") {
      const doc = records.get(params.id);
      doc.meeting.summary.text = "Updated by AI"; doc.meeting.revision++;
      doc.summary_html = "<p>Updated by AI</p>";
      return structuredClone(doc);
    }
    if (method === "meetings.update") {
      writes.push(structuredClone(params));
      const doc = records.get(params.id);
      assert.equal(params.revision, doc.meeting.revision);
      if (params.title) doc.meeting.title = params.title;
      if (params.transcript) doc.transcript = params.transcript;
      if (params.summary) { doc.meeting.summary.text = params.summary; doc.summary_html = `<p>${params.summary}</p>`; }
      doc.meeting.revision++;
      return structuredClone({ ...doc.meeting, summary_html: doc.summary_html });
    }
    if (method === "audio.stop") { stops.push(params.id); phase = "finishing"; return { accepted: true }; }
    if (method === "audio.status") return { state: { id: "live", phase, started_at: 1000 }, segments: [], partials: {}, reload: false };
    throw new Error(`Unexpected method ${method}`);
  };
  vm.runInNewContext(script, {
    meetingsRequest: call,
    document: { getElementById: get, createElement: () => new Element(), createTextNode: (text) => text, body: new Element(), addEventListener() {} },
    addEventListener() {}, getSelection: () => null, setInterval: (fn) => intervals.push(fn),
  });
  await setImmediate();
  const open = async (id) => {
    const button = get("list").children.find((button) => button.children[0]?.textContent === id);
    assert.ok(button, `Missing meeting ${id}`);
    return button.onclick();
  };
  const input = (id, text) => { get(id).value = text; get(id).oninput(); };
  return { get, open, input, pending, writes, stops, records, tick: () => intervals[0](), phase: (value) => { phase = value; } };
}

test("delayed document reads preserve newer input", async () => {
  const ui = await harness();
  await ui.open("A");
  const read = deferred(); ui.pending.set("meetings.read:B", read);
  const navigation = ui.open("B");
  ui.input("editor", "My new unsaved words");
  read.resolve(); await navigation;
  assert.equal(ui.get("title").value, "A");
  assert.equal(ui.get("editor").value, "My new unsaved words");
  assert.equal(ui.get("save").style.display, "inline-block");
  await ui.get("save").onclick();

});

test("saving summary Markdown refreshes the exact saved preview", async () => {
  const ui = await harness(); await ui.open("A");
  await ui.get("show-summary").onclick(); await ui.get("mode-edit").onclick();
  ui.input("editor", "Corrected summary 🦊");
  await ui.get("show-transcript").onclick(); await ui.get("show-summary").onclick();
  assert.equal(ui.get("preview").innerHTML, "<p>Corrected summary 🦊</p>");
  assert.equal(ui.writes[0].summary, "Corrected summary 🦊");
  assert.equal(ui.writes[0].transcript, undefined);
  assert.equal(ui.records.get("A").transcript, "A transcript");
});

test("record controls distinguish preparing, recording and finishing", async () => {
  const ui = await harness();
  await ui.get("record").onclick(); await ui.tick();
  assert.match(ui.get("metadata").textContent, /^Preparing ·/);
  ui.phase("recording"); await ui.tick();
  assert.match(ui.get("metadata").textContent, /^Recording ·/);
  await ui.get("record").onclick();
  assert.deepEqual(ui.stops, ["live"]);
  assert.equal(ui.get("record").disabled, true);
  assert.match(ui.get("metadata").textContent, /^Finishing ·/);
});

test("summary completion reconciles the saved revision before the next edit", async () => {
  const ui = await harness(); await ui.open("A");
  const response = deferred(); ui.pending.set("meetings.summarize:A", response);
  const summarizing = ui.get("summarize").onclick();
  await setImmediate();
  response.resolve(); await summarizing;
  ui.input("title", "After summary");
  await ui.get("save").onclick();
  assert.equal(ui.writes[0].revision, 1);
  assert.equal(ui.records.get("A").meeting.title, "After summary");
  assert.equal(ui.records.get("A").meeting.summary.text, "Updated by AI");
});

test("summary preview cannot publish a stale read into another meeting", async () => {
  const ui = await harness();
  await ui.open("A");
  await ui.get("show-summary").onclick();
  await ui.get("mode-edit").onclick();
  const read = deferred();
  ui.pending.set("meetings.read:A", read);
  const preview = ui.get("mode-edit").onclick();
  await setImmediate();
  await ui.open("B");
  read.resolve();
  await preview;
  assert.equal(ui.get("title").value, "B");
  assert.equal(ui.get("editor").value, "B transcript");
  ui.input("editor", "B corrected");
  await ui.get("save").onclick();
  assert.deepEqual(ui.writes, [{id: "B", revision: 0, transcript: "B corrected"}]);
});

test("record startup keeps a newer draft and still exposes Stop", async () => {
  const ui = await harness();
  await ui.open("A");
  const start = deferred();
  ui.pending.set("meetings.start:", start);
  const starting = ui.get("record").onclick();
  ui.input("editor", "Words typed during setup");
  ui.input("title", "New title");
  start.resolve();
  await starting;
  assert.equal(ui.get("title").value, "New title");
  assert.equal(ui.get("editor").value, "Words typed during setup");
  assert.equal(ui.get("save").style.display, "inline-block");
  assert.equal(ui.get("record-mobile").textContent, "Stop");
  await ui.get("save").onclick();
  assert.deepEqual(ui.writes, [{id: "A", revision: 0, title: "New title", transcript: "Words typed during setup"}]);
  await ui.get("record").onclick();
  assert.deepEqual(ui.stops, ["live"]);
});

test("record startup leaves a document opened during preparation selected", async () => {
  const ui = await harness();
  await ui.open("A");
  const start = deferred();
  ui.pending.set("meetings.start:", start);
  const starting = ui.get("record").onclick();
  await ui.open("B");
  start.resolve();
  await starting;
  assert.equal(ui.get("title").value, "B");
  assert.equal(ui.get("editor").value, "B transcript");
  assert.equal(ui.get("record-mobile").textContent, "Stop");
});
