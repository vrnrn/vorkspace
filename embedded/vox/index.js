// WebRTC media only. Rust owns the call, tools, memory and provider process.
import {
  captureVoxMicrophone,
  listVoxMicrophones,
  saveVoxMicrophone,
} from "./microphone.js";

let stopped = false, connected = false, stream, peer, channel, audio;
async function post(event) {
  const response = await fetch("./event", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(event),
  });
  if (!response.ok) throw new Error("Native voice controller disconnected");
}
function stop() {
  if (stopped) return;
  stopped = true;
  stream?.getTracks().forEach((track) => track.stop());
  if (channel) {
    channel.onclose = null;
    channel.onerror = null;
    channel.close();
  }
  if (peer) {
    peer.onconnectionstatechange = null;
    peer.close();
  }
  if (audio) {
    audio.pause();
    audio.srcObject = null;
  }
}
function fail(error) {
  if (stopped) return;
  stop();
  void post({
    type: "error",
    message: String(error?.message ?? error).slice(0, 500),
  }).catch(() => {});
}
async function controls() {
  while (!stopped) {
    const response = await fetch("./events");
    if (!response.ok) throw new Error("Native voice controller disconnected");
    const command = await response.json();
    if (stopped) return;
    if (command?.type === "stop") { stop(); return; }
    if (command?.type === "silent" && audio) audio.muted = !!command.value;
    if (command?.type === "answer") {
      await peer.setRemoteDescription({ type: "answer", sdp: command.sdp });
    }
  }
}
function ready() {
  if (!stopped && !connected && peer.connectionState === "connected" && channel.readyState === "open") {
    connected = true;
    void post({ type: "connected" }).catch(fail);
  }
}
async function start() {
  const response = await fetch("./config");
  if (!response.ok) throw new Error("Native voice controller disconnected");
  const config = await response.json();
  if (stopped) return;
  saveVoxMicrophone(config.microphone);
  const captured = await captureVoxMicrophone(() => !stopped);
  if (stopped) { captured.getTracks().forEach((track) => track.stop()); return; }
  stream = captured;
  peer = new RTCPeerConnection();
  audio = new Audio();
  audio.autoplay = true;
  audio.muted = !!config.silent;
  for (const track of stream.getTracks()) {
    track.onended = () => fail(new Error("The selected microphone disconnected"));
    peer.addTrack(track, stream);
  }
  peer.ontrack = (event) => {
    if (stopped) return;
    audio.srcObject = event.streams[0] ?? new MediaStream([event.track]);
    void audio.play().catch(() => fail(new Error("Voice playback is unavailable")));
  };
  peer.onconnectionstatechange = () => {
    if (["failed", "disconnected", "closed"].includes(peer.connectionState)) {
      fail(new Error("Voice audio disconnected"));
    } else ready();
  };
  channel = peer.createDataChannel("oai-events");
  channel.onopen = ready;
  channel.onclose = () => fail(new Error("Voice channel closed"));
  channel.onerror = () => fail(new Error("Voice channel failed"));
  channel.onmessage = (event) => {
    if (stopped || typeof event.data !== "string" || event.data.length > 64000) return;
    try {
      const value = JSON.parse(event.data);
      // Forward identities/times, never speech text or arbitrary RTC payloads.
      if (value.type === "output_transcript.added") {
        void post({ type: "activity", event: { type: value.type, start_ms: value.start_ms } }).catch(fail);
      }
      if (["turn.created", "turn.done"].includes(value.type) && value.turn) {
        const { id, role, start_ms, end_ms } = value.turn;
        void post({ type: "activity", event: { type: value.type, turn: { id, role, start_ms, end_ms } } }).catch(fail);
      }
    } catch { /* Unsupported media control frames do not interrupt audio. */ }
  };
  const offer = await peer.createOffer();
  if (stopped) return;
  await peer.setLocalDescription(offer);
  if (stopped) return;
  void controls().catch(fail);
  await post({ type: "offer", sdp: offer.sdp });
  const devices = await listVoxMicrophones();
  if (!stopped) {
    await post({ type: "devices", devices: devices.slice(0, 64).map((device) => ({
      id: device.deviceId.slice(0, 256), label: device.label.slice(0, 256),
    })) });
  }
}
window.addEventListener("pagehide", stop);
void start().catch(fail);
