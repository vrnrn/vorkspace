import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { captureVoxMicrophone, listVoxMicrophones, saveVoxMicrophone } from "./microphone.js";
vi.mock("./microphone.js", () => ({
  captureVoxMicrophone: vi.fn(), listVoxMicrophones: vi.fn(), saveVoxMicrophone: vi.fn(),
}));
let hide, track, stream, peer, channel, audio, posts, polls;
const reply = (value) => ({ ok: true, json: async () => value });
beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  posts = []; polls = [];
  track = { stop: vi.fn() };
  stream = { getTracks: () => [track] };
  captureVoxMicrophone.mockResolvedValue(stream);
  listVoxMicrophones.mockResolvedValue([{ deviceId: "built-in", label: "Built-in Microphone" }]);
  channel = { close: vi.fn(), readyState: "connecting" };
  peer = {
    addTrack: vi.fn(), close: vi.fn(), connectionState: "new",
    createDataChannel: vi.fn(() => channel),
    createOffer: vi.fn(async () => ({ type: "offer", sdp: "local-sdp" })),
    setLocalDescription: vi.fn(async () => {}), setRemoteDescription: vi.fn(async () => {}),
  };
  audio = { play: vi.fn(async () => {}), pause: vi.fn() };
  vi.stubGlobal("RTCPeerConnection", vi.fn(function () { return peer; }));
  vi.stubGlobal("Audio", vi.fn(function () { return audio; }));
  vi.stubGlobal("window", { addEventListener: vi.fn((type, fn) => { if (type === "pagehide") hide = fn; }) });
  vi.stubGlobal("fetch", vi.fn(async (url, options) => {
    if (url === "./config") return reply({ microphone: { id: "built-in" }, silent: true });
    if (url === "./event") { posts.push(JSON.parse(options.body)); return reply({ ok: true }); }
    if (url === "./events") return new Promise((resolve) => polls.push((value) => resolve(reply(value))));
    throw new Error(`Unexpected route: ${url}`);
  }));
});
afterEach(() => { hide?.(); polls.forEach((resolve) => resolve(null)); vi.unstubAllGlobals(); });
async function start() {
  await import("./index.js");
  await vi.waitFor(() => expect(posts.some((event) => event.type === "offer")).toBe(true));
}
it("exchanges only media SDP and stops all local media on End", async () => {
  await start();
  expect(saveVoxMicrophone).toHaveBeenCalledWith({ id: "built-in" });
  expect(audio.muted).toBe(true);
  expect(posts).toContainEqual({ type: "offer", sdp: "local-sdp" });
  polls.shift()({ type: "answer", sdp: "remote-sdp" });
  await vi.waitFor(() => expect(peer.setRemoteDescription).toHaveBeenCalledWith({ type: "answer", sdp: "remote-sdp" }));
  polls.shift()({ type: "silent", value: false });
  await vi.waitFor(() => expect(audio.muted).toBe(false));
  polls.shift()({ type: "stop" });
  await vi.waitFor(() => expect(track.stop).toHaveBeenCalledOnce());
  expect(peer.close).toHaveBeenCalledOnce();
  expect(channel.close).toHaveBeenCalledOnce();
  expect(audio.pause).toHaveBeenCalledOnce();
  expect(audio.srcObject).toBeNull();
});
it("releases a late capture without opening a peer after page disposal", async () => {
  let grant;
  captureVoxMicrophone.mockImplementation(() => new Promise((resolve) => { grant = resolve; }));
  await import("./index.js");
  await vi.waitFor(() => expect(grant).toBeTypeOf("function"));
  hide(); grant(stream);
  await vi.waitFor(() => expect(track.stop).toHaveBeenCalledOnce());
  expect(RTCPeerConnection).not.toHaveBeenCalled();
  expect(posts).toEqual([]);
});
it("forwards playback identity without speech or tool payloads and reports connection once", async () => {
  await start();
  channel.onmessage({ data: JSON.stringify({ type: "output_transcript.added", start_ms: 10, text: "private speech" }) });
  channel.onmessage({ data: JSON.stringify({ type: "turn.done", turn: { id: "a", role: "assistant", start_ms: 10, end_ms: 20, text: "private" } }) });
  channel.onmessage({ data: JSON.stringify({ type: "item/tool/call", arguments: { action: "sessions.send" } }) });
  peer.connectionState = "connected"; channel.readyState = "open";
  peer.onconnectionstatechange(); channel.onopen();
  expect(posts.filter((event) => event.type === "connected")).toHaveLength(1);
  expect(posts.filter((event) => event.type === "activity")).toEqual([
    { type: "activity", event: { type: "output_transcript.added", start_ms: 10 } },
    { type: "activity", event: { type: "turn.done", turn: { id: "a", role: "assistant", start_ms: 10, end_ms: 20 } } },
  ]);
  expect(JSON.stringify(posts)).not.toContain("private");
  expect(JSON.stringify(posts)).not.toContain("sessions.send");
});
it("microphone loss closes playback and reports one error without reconnecting", async () => {
  await start();
  track.onended(); track.onended();
  expect(track.stop).toHaveBeenCalledOnce();
  expect(peer.close).toHaveBeenCalledOnce();
  expect(posts.filter((event) => event.type === "error")).toHaveLength(1);
  expect(captureVoxMicrophone).toHaveBeenCalledOnce();
});
