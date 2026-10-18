import { afterEach, expect, it, vi } from "vitest";
import { VoxDisplay } from "./display";
import { voxFace, VOX_FACES, VOX_MOODS } from "./expressions";
const displays: VoxDisplay[] = [];
function create() {
  const store = new VoxDisplay();
  displays.push(store);
  return store;
}
function done(store: VoxDisplay, role: string, text: string) {
  store.notification("thread/realtime/transcript/done", { role, text });
}
afterEach(() => {
  displays.splice(0).forEach((store) => store.dispose());
  vi.useRealTimers();
});

it("ignores partial speech and user text without allocating captions or notifying the panel", () => {
  vi.useFakeTimers();
  const store = create();
  const initial = store.getSnapshot();
  const notify = vi.fn();
  store.subscribe(notify);
  for (let n = 0; n < 100; n++) {
    store.notification("thread/realtime/transcript/delta", {
      role: "user",
      delta: "spoken word",
    });
    store.notification("thread/realtime/transcript/delta", {
      role: "assistant",
      delta: "partial reply",
    });
  }
  done(store, "user", "private spoken request");
  expect(store.getSnapshot()).toBe(initial);
  expect(notify).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it("publishes final replies immediately and runs no work for a closed panel", () => {
  vi.useFakeTimers();
  const store = create();
  const notify = vi.fn();
  const close = store.subscribe(notify);
  done(store, "assistant", "OriginalCase.ts is ready");
  expect(notify).toHaveBeenCalledOnce();
  expect(store.getSnapshot().lines[0].text).toBe("OriginalCase.ts is ready");
  close();
  done(store, "assistant", "another reply");
  expect(store.getSnapshot().lines.map((line) => line.text)).toEqual([
    "another reply",
  ]);
  expect(notify).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it("replaces the last native reply, including repeats, and rejects empty or duplicate event families", () => {
  const store = create();
  done(store, "assistant", "again");
  const first = store.getSnapshot().lines[0].id;
  done(store, "assistant", "again");
  done(store, "assistant", " ");
  done(store, "unknown", "wrong role");
  store.notification("thread/realtime/item/completed", {
    item: {
      type: "transcriptSegment",
      role: "assistant",
      text: "duplicate family",
    },
  });
  expect(store.getSnapshot().lines.map((line) => line.text)).toEqual(["again"]);
  expect(store.getSnapshot().lines[0].id).not.toBe(first);
});

it("retains only the latest bounded reply and clears it on a new call", () => {
  const store = create();
  for (let n = 0; n < 30; n++)
    done(store, "assistant", `${n} ${"x".repeat(20_000)}`);
  const { lines } = store.getSnapshot();
  expect(lines).toHaveLength(1);
  expect(lines[0].text).toMatch(/^29 /);
  expect(
    lines.every((line) => line.text.length <= 4_000 && line.truncated),
  ).toBe(true);
  store.reset();
  expect(store.getSnapshot().lines).toEqual([]);
});

it("keeps actual activity authoritative over model moods and clears moods without keeping user text", () => {
  const store = create();
  store.mood("happy");
  store.notification("turn/started", {});
  const controls = { state: "live" as const, error: null, micEnabled: true };
  expect(voxFace({ ...controls, ...store.getSnapshot() })).toBe("thinking");
  expect(voxFace({ ...controls, ...store.getSnapshot(), error: "lost" })).toBe(
    "error",
  );
  expect(
    voxFace({ ...controls, ...store.getSnapshot(), micEnabled: false }),
  ).toBe("paused");
  store.notification("turn/completed", {});
  done(store, "user", "next request");
  expect(store.getSnapshot().mood).toBe("neutral");
  expect(store.getSnapshot().lines).toEqual([]);
  expect(voxFace({ ...controls, ...store.getSnapshot() })).toBe("listening");
  expect(Object.keys(VOX_FACES)).toHaveLength(16);
  expect(VOX_MOODS).not.toContain("error");
  expect(
    Object.values(VOX_FACES).every(
      (face) => face.face === face.face.toLowerCase(),
    ),
  ).toBe(true);
});

it("tracks native playback boundaries without letting old replies or backing-agent completion stop a newer speaking face", () => {
  const store = create();
  store.realtime({
    type: "turn.created",
    turn: { id: "old", role: "assistant", start_ms: 0, end_ms: 200 },
  });
  expect(store.getSnapshot().activity).toBe("replying");
  store.notification("turn/completed", {});
  expect(store.getSnapshot().activity).toBe("replying");
  store.realtime({
    type: "turn.created",
    turn: { id: "user", role: "user", start_ms: 200, end_ms: 400 },
  });
  expect(store.getSnapshot().activity).toBe("listening");
  store.realtime({
    type: "turn.done",
    turn: { id: "user", role: "user", start_ms: 200, end_ms: 800 },
  });
  expect(store.getSnapshot().activity).toBe("thinking");
  store.realtime({ type: "output_transcript.added", start_ms: 1000 });
  store.realtime({
    type: "turn.done",
    turn: { id: "old", role: "assistant", start_ms: 0, end_ms: 200 },
  });
  store.notification("turn/completed", {});
  expect(store.getSnapshot().activity).toBe("replying");
  store.realtime({
    type: "turn.done",
    turn: { id: "new", role: "assistant", start_ms: 1000, end_ms: 2000 },
  });
  expect(store.getSnapshot().activity).toBe("listening");
  store.realtime({
    type: "turn.created",
    turn: { id: "last", role: "assistant", start_ms: 2000, end_ms: 2200 },
  });
  store.finish();
  expect(store.getSnapshot().activity).toBe("listening");
});
