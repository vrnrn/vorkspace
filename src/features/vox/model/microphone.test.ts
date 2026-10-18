import { beforeEach, afterEach, expect, it, vi } from "vitest";
import {
  captureVoxMicrophone,
  loadVoxMicrophone,
  saveVoxMicrophone,
} from "./microphone";
const devices = [
  { kind: "audioinput", deviceId: "airpods", label: "AirPods Pro" },
  { kind: "audioinput", deviceId: "macbook", label: "MacBook Pro Microphone" },
];
let capture: ReturnType<typeof vi.fn>;
let enumerate: ReturnType<typeof vi.fn>;
const makeStream = () => {
  const track = { stop: vi.fn() };
  return { getTracks: () => [track], track };
};
beforeEach(() => {
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  });
  capture = vi.fn(async () => makeStream());
  enumerate = vi.fn(async () => devices);
  vi.stubGlobal("navigator", {
    mediaDevices: { getUserMedia: capture, enumerateDevices: enumerate },
  });
});
afterEach(() => vi.unstubAllGlobals());
it("defaults to a pinned built-in input even when AirPods is the system default", async () => {
  await captureVoxMicrophone(() => true);
  expect(capture).toHaveBeenCalledExactlyOnceWith({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      deviceId: { exact: "macbook" },
    },
    video: false,
  });
});
it("persists an explicit default and uses it on the next capture", async () => {
  saveVoxMicrophone({ id: "airpods", label: "AirPods Pro" });
  expect(loadVoxMicrophone()).toEqual({ id: "airpods", label: "AirPods Pro" });
  await captureVoxMicrophone(() => true);
  expect(capture.mock.calls[0][0].audio.deviceId).toEqual({ exact: "airpods" });
});
it("uses the system input only when selected", async () => {
  saveVoxMicrophone({ id: "system", label: "system default" });
  await captureVoxMicrophone(() => true);
  expect(enumerate).not.toHaveBeenCalled();
  expect(capture.mock.calls[0][0].audio).not.toHaveProperty("deviceId");
});
it("does not fall back when the selected microphone is missing", async () => {
  enumerate.mockResolvedValue([devices[0]]);
  await expect(captureVoxMicrophone(() => true)).rejects.toThrow(
    "settings → vox",
  );
  expect(capture).not.toHaveBeenCalled();
});
it("reports a device removed after discovery without falling back", async () => {
  capture.mockRejectedValueOnce(
    new DOMException("gone", "OverconstrainedError"),
  );
  await expect(captureVoxMicrophone(() => true)).rejects.toThrow(
    "settings → vox",
  );
  expect(capture).toHaveBeenCalledOnce();
});
it("releases permission-only capture before opening the pinned device", async () => {
  const permission = makeStream();
  capture.mockResolvedValueOnce(permission);
  enumerate
    .mockResolvedValueOnce([{ ...devices[0], label: "" }])
    .mockResolvedValueOnce(devices);
  await captureVoxMicrophone(() => true);
  expect(permission.track.stop).toHaveBeenCalledOnce();
  expect(capture).toHaveBeenCalledTimes(2);
  expect(capture.mock.calls[1][0].audio.deviceId).toEqual({ exact: "macbook" });
});
it("releases a late permission grant after cancellation without starting another capture", async () => {
  let grant!: (stream: ReturnType<typeof makeStream>) => void;
  const permission = makeStream();
  let current = true;
  capture.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        grant = resolve;
      }),
  );
  enumerate.mockResolvedValueOnce([{ ...devices[0], label: "" }]);
  const pending = captureVoxMicrophone(() => current);
  await vi.waitFor(() => expect(capture).toHaveBeenCalledOnce());
  current = false;
  grant(permission);
  await expect(pending).rejects.toThrow("cancelled");
  expect(permission.track.stop).toHaveBeenCalledOnce();
  expect(capture).toHaveBeenCalledOnce();
});
