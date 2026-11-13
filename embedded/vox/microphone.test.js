import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { captureVoxMicrophone } from "./microphone.js";
const devices = [
  { kind: "audioinput", deviceId: "airpods", label: "AirPods Pro" },
  { kind: "audioinput", deviceId: "macbook", label: "MacBook Pro Microphone" },
];
let capture;
let enumerate;
const makeStream = () => {
  const track = { stop: vi.fn() };
  return { getTracks: () => [track], track };
};
beforeEach(() => {
  capture = vi.fn(async () => makeStream());
  enumerate = vi.fn(async () => devices);
  vi.stubGlobal("navigator", {
    mediaDevices: { getUserMedia: capture, enumerateDevices: enumerate },
  });
});
afterEach(() => vi.unstubAllGlobals());
it("defaults to a pinned built-in input even when AirPods is the system default", async () => {
  await captureVoxMicrophone(undefined, () => true);
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
it("pins the input selected in native settings", async () => {
  await captureVoxMicrophone({ id: "airpods", label: "AirPods Pro" }, () => true);
  expect(capture.mock.calls[0][0].audio.deviceId).toEqual({ exact: "airpods" });
});
it("uses the system input only when selected", async () => {
  await captureVoxMicrophone({ id: "system", label: "system default" }, () => true);
  expect(enumerate).not.toHaveBeenCalled();
  expect(capture.mock.calls[0][0].audio).not.toHaveProperty("deviceId");
});
it("does not fall back when the selected microphone is missing", async () => {
  enumerate.mockResolvedValue([devices[0]]);
  await expect(captureVoxMicrophone(undefined, () => true)).rejects.toThrow(
    "settings → vox",
  );
  expect(capture).not.toHaveBeenCalled();
});
it("reports a device removed after discovery without falling back", async () => {
  capture.mockRejectedValueOnce(
    new DOMException("gone", "OverconstrainedError"),
  );
  await expect(captureVoxMicrophone(undefined, () => true)).rejects.toThrow(
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
  await captureVoxMicrophone(undefined, () => true);
  expect(permission.track.stop).toHaveBeenCalledOnce();
  expect(capture).toHaveBeenCalledTimes(2);
  expect(capture.mock.calls[1][0].audio.deviceId).toEqual({ exact: "macbook" });
});
it("releases a late permission grant after cancellation without starting another capture", async () => {
  let grant;
  const permission = makeStream();
  let current = true;
  capture.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        grant = resolve;
      }),
  );
  enumerate.mockResolvedValueOnce([{ ...devices[0], label: "" }]);
  const pending = captureVoxMicrophone(undefined, () => current);
  await vi.waitFor(() => expect(capture).toHaveBeenCalledOnce());
  current = false;
  grant(permission);
  await expect(pending).rejects.toThrow("cancelled");
  expect(permission.track.stop).toHaveBeenCalledOnce();
  expect(capture).toHaveBeenCalledOnce();
});
it("recognizes a uniquely selected device after the private origin rotates its ID", async () => {
  await captureVoxMicrophone(
    { id: "old-id", label: "MacBook Pro Microphone" }, () => true,
  );
  expect(capture.mock.calls[0][0].audio.deviceId).toEqual({ exact: "macbook" });
});
it("refuses ambiguous labels after IDs rotate", async () => {
  enumerate.mockResolvedValue([
    { kind: "audioinput", deviceId: "one", label: "USB Mic" },
    { kind: "audioinput", deviceId: "two", label: "USB Mic" },
  ]);
  await expect(
    captureVoxMicrophone({ id: "old-id", label: "USB Mic" }, () => true),
  ).rejects.toThrow("unavailable");
  expect(capture).not.toHaveBeenCalled();
});
