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
  expect(store.getSnapshot().lines).toHaveLength(2);
  expect(notify).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it("keeps repeated native replies and rejects empty or duplicate event families", () => {
  const store = create();
  done(store, "assistant", "again");
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
  expect(store.getSnapshot().lines.map((line) => line.text)).toEqual([
    "again",
    "again",
  ]);
});

it("bounds retained reply count and text cost and clears them on a new call", () => {
  const store = create();
  for (let n = 0; n < 30; n++) done(store, "assistant", "x".repeat(20_000));
  const { lines } = store.getSnapshot();
  expect(lines.length).toBeLessThanOrEqual(12);
  expect(
    lines.reduce((sum, line) => sum + line.text.length, 0),
  ).toBeLessThanOrEqual(16_000);
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
