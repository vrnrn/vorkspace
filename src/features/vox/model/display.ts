import type { VoxMood } from "./expressions";

export type VoxTranscript = {
  id: string;
  role: "assistant";
  text: string;
  done: true;
  truncated: boolean;
};
type Snapshot = {
  lines: readonly VoxTranscript[];
  activity: "listening" | "thinking" | "replying";
  mood: VoxMood;
};
const MAX_LINES = 12;
const MAX_LINE_CHARS = 4_000;
const MAX_CHARS = 16_000;

/** Native final replies only. Partial speech is suppressed before IPC, user
 * transcripts are discarded, and the closed panel has no observers or timers.
 * Nothing is persisted; reads and notifications share one current snapshot. */
export class VoxDisplay {
  private snapshot: Snapshot = {
    lines: [],
    activity: "listening",
    mood: "neutral",
  };
  private listeners = new Set<() => void>();
  private serial = 0;
  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  private publish(next: Snapshot) {
    this.snapshot = next;
    for (const listener of this.listeners) listener();
  }

  reset() {
    this.publish({ lines: [], activity: "listening", mood: "neutral" });
  }

  finish() {
    this.publish({ ...this.snapshot, activity: "listening" });
  }

  mood(mood: VoxMood) {
    if (this.snapshot.mood !== mood) this.publish({ ...this.snapshot, mood });
  }

  notification(method: string, value: Record<string, unknown>) {
    if (method === "turn/started" || method === "turn/completed") {
      const activity = method === "turn/started" ? "thinking" : "listening";
      if (this.snapshot.activity !== activity)
        this.publish({ ...this.snapshot, activity });
      return;
    }
    if (
      method !== "thread/realtime/transcript/done" ||
      typeof value.text !== "string"
    )
      return;
    if (value.role === "user") {
      this.mood("neutral");
      return;
    }
    if (value.role !== "assistant" || !value.text.trim()) return;
    const line: VoxTranscript = {
      id: `reply-${++this.serial}`,
      role: "assistant",
      text: value.text.slice(0, MAX_LINE_CHARS),
      done: true,
      truncated: value.text.length > MAX_LINE_CHARS,
    };
    let lines = [...this.snapshot.lines, line];
    let size = lines.reduce((sum, entry) => sum + entry.text.length, 0);
    while (lines.length > MAX_LINES || size > MAX_CHARS) {
      size -= lines[0].text.length;
      lines = lines.slice(1);
    }
    this.publish({ ...this.snapshot, lines });
  }

  dispose() {
    this.listeners.clear();
  }
}
