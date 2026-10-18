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
const MAX_LINE_CHARS = 4_000;

/** Native final replies and coarse playback activity only. Partial speech is
 * suppressed before IPC, user transcripts are discarded, and no timers run.
 * Nothing is persisted; reads and notifications share one current snapshot. */
export class VoxDisplay {
  private snapshot: Snapshot = {
    lines: [],
    activity: "listening",
    mood: "neutral",
  };
  private listeners = new Set<() => void>();
  private serial = 0;
  private thinking = false;
  private speaking: string | null = null;
  private speakingAt = -1;
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
    this.thinking = false;
    this.speaking = null;
    this.speakingAt = -1;
    this.publish({ lines: [], activity: "listening", mood: "neutral" });
  }

  finish() {
    this.thinking = false;
    this.speaking = null;
    this.speakingAt = -1;
    this.publish({ ...this.snapshot, activity: "listening" });
  }

  mood(mood: VoxMood) {
    if (this.snapshot.mood !== mood) this.publish({ ...this.snapshot, mood });
  }

  private activity(mood = this.snapshot.mood) {
    const activity = this.speaking
      ? "replying"
      : this.thinking
        ? "thinking"
        : "listening";
    if (this.snapshot.activity !== activity || this.snapshot.mood !== mood)
      this.publish({ ...this.snapshot, activity, mood });
  }

  /** Codex live voice's native channel emits turn and output-transcript
   * boundaries. Observe their identities/timestamps, never their speech text. */
  realtime(value: Record<string, unknown>) {
    let mood = this.snapshot.mood;
    if (value.type === "output_transcript.added") {
      if (typeof value.start_ms !== "number") return;
      this.speaking ??= "pending";
      this.speakingAt = Math.max(this.speakingAt, value.start_ms);
    } else if (value.type === "turn.created" || value.type === "turn.done") {
      const turn = value.turn;
      if (!turn || typeof turn !== "object") return;
      const {
        id,
        role,
        start_ms: start,
        end_ms: end,
      } = turn as Record<string, unknown>;
      if (
        typeof id !== "string" ||
        typeof start !== "number" ||
        typeof end !== "number"
      )
        return;
      if (role === "user") {
        if (value.type === "turn.created") {
          this.speaking = null;
          this.thinking = false;
          mood = "neutral";
        } else this.thinking = true;
      } else if (role === "assistant") {
        // Old completion/creation cannot overwrite speech from a later turn.
        if (end < this.speakingAt) return;
        if (value.type === "turn.created") {
          this.speaking = id;
          this.speakingAt = Math.max(this.speakingAt, start);
        } else {
          if (this.speaking !== id && this.speaking !== "pending") return;
          this.speaking = null;
          this.thinking = false;
        }
      } else return;
    } else return;
    this.activity(mood);
  }

  notification(method: string, value: Record<string, unknown>) {
    if (method === "turn/started" || method === "turn/completed") {
      this.thinking = method === "turn/started";
      this.activity();
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
    this.publish({ ...this.snapshot, lines: [line] });
  }

  dispose() {
    this.listeners.clear();
  }
}
