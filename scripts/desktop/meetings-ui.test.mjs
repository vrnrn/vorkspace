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
  get firstElementChild() { return this.children[0]; }
  get lastElementChild() { return this.children.at(-1); }
  replaceChildren() { for (const child of [...this.children]) if (child instanceof Element) child.remove(); this.children = []; }
  remove() {
    if (!this.parent) return;
    this.parent.children.splice(this.parent.children.indexOf(this), 1);
    this.parent = null;
  }
  insertBefore(child, next) {
    if (child instanceof Element) { child.remove(); child.parent = this; }
    this.children.splice(next ? this.children.indexOf(next) : this.children.length, 0, child);
  }
  append(...children) { for (const child of children) this.insertBefore(child, null); }
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
  const readResponses = new Map(), pending = new Map(), summaryResponses = new Map(), statusResponses = new Map(), writes = [], stops = [];
  let phase = "starting", feed = {};
  const call = async (method, params = {}) => {
    if (pending.has(method + ":" + (params.id ?? ""))) await pending.get(method + ":" + (params.id ?? "")).promise;
    if (method === "meetings.list") return [...records.values()].map(({ meeting, state }) => structuredClone({ meeting, state }));
    if (method === "meetings.read") {
      const snapshot = structuredClone(records.get(params.id));
      await readResponses.get(params.id)?.promise;
      return snapshot;
    }
    if (method === "meetings.start") return { id: "live", state: { id: "live", phase: "starting", started_at: 1000 } };
    if (method === "meetings.summarize") {
      const doc = records.get(params.id);
      doc.meeting.summary.text = "Updated by AI"; doc.meeting.revision++;
      doc.summary_html = "<p>Updated by AI</p>";
      const snapshot = structuredClone(doc);
      await summaryResponses.get(params.id)?.promise;
      return snapshot;
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
    if (method === "audio.current") return null;
    if (method === "audio.stop") { stops.push(params.id); phase = "finishing"; return { accepted: true }; }
    if (method === "audio.status") {
      const snapshot = { state: { id: "live", phase, started_at: 1000 }, segments: [], partials: {}, reload: false, ...feed };
      await statusResponses.get(params.id)?.promise;
      return snapshot;
    }
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
  return { get, open, input, pending, readResponses, summaryResponses, statusResponses, writes, stops, records, tick: () => intervals[0](), phase: (value) => { phase = value; }, feed: (value) => { feed = value; } };
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

test("reselecting the current meeting supersedes a pending navigation", async () => {
  const ui = await harness();
  await ui.open("A");
  const read = deferred();
  ui.pending.set("meetings.read:B", read);
  const navigation = ui.open("B");
  await ui.open("A");
  read.resolve();
  await navigation;
  assert.equal(ui.get("title").value, "A");
  assert.equal(ui.get("editor").value, "A transcript");
});

test("a delayed summary response cannot roll back a reopened meeting's saved revision", async () => {
  const ui = await harness();
  await ui.open("A");
  const response = deferred();
  ui.summaryResponses.set("A", response);
  const summary = ui.get("summarize").onclick();
  await setImmediate();
  assert.equal(ui.records.get("A").meeting.revision, 1);
  await ui.open("B");
  await ui.open("A");
  ui.input("editor", "Newly saved transcript");
  await ui.get("save").onclick();
  assert.equal(ui.records.get("A").meeting.revision, 2);

  response.resolve();
  await summary;
  await ui.get("show-summary").onclick();
  await ui.get("show-transcript").onclick();
  assert.equal(ui.get("editor").value, "Newly saved transcript");
  ui.input("title", "Newer title");
  await ui.get("save").onclick();
  assert.equal(ui.writes.at(-1).revision, 2);
  assert.equal(ui.records.get("A").meeting.title, "Newer title");
});

test("a delayed Stop reply preserves an already observed terminal phase", async () => {
  const ui = await harness();
  await ui.get("record").onclick();
  ui.phase("recording");
  await ui.tick();
  const reply = deferred();
  ui.pending.set("audio.stop:live", reply);
  const stopping = ui.get("record").onclick();
  ui.records.set("live", document("live"));
  ui.phase("stopped");
  await ui.tick();
  reply.resolve();
  await stopping;
  assert.equal(ui.get("record-mobile").textContent, "Record");
  assert.equal(ui.get("record").disabled, false);
  await ui.get("record").onclick();
  assert.equal(ui.get("record-mobile").textContent, "Stop");
});

test("a poll started before Stop cannot regress Finishing to Recording", async () => {
  const ui = await harness();
  await ui.get("record").onclick();
  ui.phase("recording");
  await ui.tick();
  const reply = deferred();
  ui.statusResponses.set("live", reply);
  const poll = ui.tick();
  await setImmediate();
  await ui.get("record").onclick();
  reply.resolve();
  await poll;
  assert.equal(ui.get("record-mobile").textContent, "Finishing…");
  assert.equal(ui.get("record").disabled, true);
});

test("summary completion refreshes the view selected during generation", async () => {
  for (const view of ["summary", "transcript"]) {
    const ui = await harness();
    await ui.open("A");
    const response = deferred();
    ui.pending.set("meetings.summarize:A", response);
    const summary = ui.get("summarize").onclick();
    await setImmediate();
    await ui.get(`show-${view}`).onclick();
    response.resolve();
    await summary;
    assert.equal(ui.get(`show-${view}`).attributes["aria-pressed"], "true");
    if (view === "summary") assert.equal(ui.get("preview").innerHTML, "<p>Updated by AI</p>");
    else assert.equal(ui.get("editor").value, "A transcript");
  }
});

test("Record is ready when an unselected recording finishes", async () => {
  const ui = await harness();
  await ui.get("record").onclick();
  ui.phase("recording");
  await ui.tick();
  await ui.open("B");
  await ui.get("record-mobile").onclick();
  assert.equal(ui.get("record-mobile").textContent, "Finishing…");
  assert.equal(ui.get("record-mobile").disabled, true);
  ui.phase("stopped");
  await ui.tick();
  assert.equal(ui.get("title").value, "B");
  assert.equal(ui.get("record-mobile").textContent, "Record");
  assert.equal(ui.get("record-mobile").disabled, false);
});

test("switching to Transcript while Stop loads it keeps the saved recording", async () => {
  const ui = await harness();
  await ui.get("record").onclick();
  ui.phase("recording");
  await ui.tick();
  ui.records.set("live", { ...document("live"), transcript: "Captured words" });
  const read = deferred();
  ui.pending.set("meetings.read:live", read);
  ui.phase("stopped");
  const terminalPoll = ui.tick();
  await setImmediate();
  await ui.get("show-transcript").onclick();
  assert.equal(ui.get("editor").readOnly, true);
  read.resolve();
  await terminalPoll;
  await setImmediate();
  assert.equal(ui.get("editor").value, "Captured words");
  assert.equal(ui.get("editor").readOnly, false);
});


test("a stale document snapshot cannot reactivate a finished recording", async () => {
  const ui = await harness();
  const live = document("live");
  live.state.phase = "recording";
  ui.records.set("live", live);
  await ui.get("record").onclick();
  ui.phase("recording"); await ui.tick(); await ui.open("A");
  const read = deferred();
  ui.readResponses.set("live", read);
  const navigation = ui.open("live");
  await setImmediate();
  live.state.phase = "stopped";
  ui.phase("stopped"); await ui.tick();
  ui.readResponses.delete("live");
  read.resolve(); await navigation;
  assert.equal(ui.get("record").attributes["aria-pressed"], "false");
  assert.equal(ui.get("editor").value, "live transcript");
});

const segment = (sequence) => ({sequence, start_ms: sequence, source: "system", text: `phrase ${sequence}`});

test("gap recovery follows the capture across view changes and bounds the retained transcript", async () => {
  const ui = await harness();
  const live = document("live");
  live.state.phase = "recording";
  live.segments = Array.from({ length: 500 }, (_, index) => segment(index + 1));
  live.sequence = 500;
  ui.records.set("live", live);
  await ui.get("record").onclick();
  ui.phase("recording"); await ui.tick();
  const read = deferred();
  ui.readResponses.set("live", read);
  ui.feed({reload: true, segments: [segment(500)]});
  const recovery = ui.tick();
  await setImmediate();
  await ui.get("show-transcript").onclick();
  read.resolve(); await recovery; await setImmediate();
  const finals = ui.get("finals");
  assert.equal(finals.children.length, 400);
  assert.equal(finals.firstElementChild.children[1], "phrase 101");

  ui.feed({segments: [segment(501)]});
  await ui.tick();
  assert.equal(finals.children.length, 400);
  assert.equal(finals.firstElementChild.children[1], "phrase 102");
  assert.equal(finals.lastElementChild.children[1], "phrase 501");
});

test("an unselected capture's failure is visible without a saved row", async () => {
  const ui = await harness();
  await ui.get("record").onclick();
  await ui.open("A");
  ui.feed({ state: { id: "live", phase: "failed", error: "Voice lease denied" } });
  await ui.tick();
  assert.equal(ui.get("notice").textContent, "Voice lease denied");
  assert.equal(ui.get("title").value, "A");
  assert.equal(ui.get("record").attributes["aria-pressed"], "false");
});

test("a title save updates the focused list row in place", async () => {
  const ui = await harness();
  await ui.open("A");
  const row = ui.get("list").children[0];
  ui.input("title", "Renamed");
  await ui.get("save").onclick();
  assert.equal(ui.get("list").children[0], row);
  assert.equal(row.children[0].textContent, "Renamed");
});

test("Stop settles while a live gap read is still waiting", async () => {
  const ui = await harness();
  const live = document("live");
  live.state.phase = "recording";
  live.segments = [segment(7)];
  live.sequence = 7;
  ui.records.set("live", live);
  await ui.get("record").onclick();
  ui.phase("recording"); await ui.tick();

  const gap = deferred();
  ui.readResponses.set("live", gap);
  ui.feed({reload: true});
  await ui.tick();
  await setImmediate();

  await ui.get("record").onclick();
  ui.readResponses.delete("live");
  live.state.phase = "stopped";
  ui.phase("stopped"); await ui.tick(); await setImmediate();
  assert.equal(ui.get("record").attributes["aria-pressed"], "false");
  assert.equal(ui.get("editor").value, "live transcript");
  gap.resolve(); await setImmediate();
  assert.equal(ui.get("editor").value, "live transcript");
  assert.equal(ui.get("finals").children.length, 0);
});
