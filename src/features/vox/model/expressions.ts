/** The glyphs are vox's artwork. Model moods never replace connection status. */
export const VOX_FACES = {
  neutral: { face: "vox", emotion: "calm" },
  happy: { face: "vov", emotion: "pleased" },
  listening: { face: "o-o", emotion: "attentive" },
  focused: { face: "-o-", emotion: "concentrating" },
  thinking: { face: "p-p", emotion: "pondering" },
  blink: { face: "-_-", emotion: "blink" },
  wink_left: { face: "vo-", emotion: "playful" },
  wink_right: { face: "-ov", emotion: "reassuring" },
  curious: { face: "o_o", emotion: "curious" },
  surprised: { face: "o!o", emotion: "surprised" },
  unsure: { face: "o.o", emotion: "uncertain" },
  concerned: { face: "v_v", emotion: "concerned" },
  delighted: { face: "^o^", emotion: "delighted" },
  cheeky: { face: ">o<", emotion: "cheeky" },
  paused: { face: "u-u", emotion: "resting" },
  error: { face: "x-x", emotion: "connection lost" },
} as const;

export const VOX_MOODS = [
  "neutral",
  "happy",
  "focused",
  "wink_left",
  "wink_right",
  "curious",
  "surprised",
  "unsure",
  "concerned",
  "delighted",
  "cheeky",
] as const;
export type VoxMood = (typeof VOX_MOODS)[number];
export type VoxFaceId = keyof typeof VOX_FACES;

export function voxFace(input: {
  state: "idle" | "connecting" | "live";
  error: string | null;
  micEnabled: boolean;
  activity: "listening" | "thinking" | "replying";
  mood: VoxMood;
}): VoxFaceId {
  if (input.error) return "error";
  if (input.state === "connecting") return "blink";
  if (input.state === "idle") return "neutral";
  if (!input.micEnabled) return "paused";
  if (input.activity === "thinking") return "thinking";
  if (input.mood !== "neutral") return input.mood;
  return input.activity === "replying" ? "happy" : "listening";
}
