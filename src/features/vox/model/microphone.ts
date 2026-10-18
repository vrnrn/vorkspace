const KEY = "vorkspace.vox.microphone.v1";
export type VoxMicrophone = { id: string; label: string };
const DEFAULT: VoxMicrophone = { id: "builtin", label: "built-in microphone" };
const AUDIO = {
  channelCount: 1,
  echoCancellation: true,
  noiseSuppression: true,
};

export function loadVoxMicrophone(): VoxMicrophone {
  try {
    const value = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (
      typeof value?.id === "string" &&
      value.id &&
      typeof value.label === "string"
    )
      return { id: value.id, label: value.label };
  } catch {
    /* use the app's default when storage is unavailable */
  }
  return DEFAULT;
}
export function saveVoxMicrophone(value: VoxMicrophone) {
  localStorage.setItem(KEY, JSON.stringify(value));
}
export async function listVoxMicrophones(): Promise<MediaDeviceInfo[]> {
  if (!navigator.mediaDevices?.enumerateDevices) return [];
  return (await navigator.mediaDevices.enumerateDevices()).filter(
    (device) =>
      device.kind === "audioinput" &&
      device.deviceId &&
      !["default", "communications"].includes(device.deviceId),
  );
}
export class VoxMicrophoneError extends Error {}
export function findVoxMicrophone(
  selected: VoxMicrophone,
  devices: MediaDeviceInfo[],
) {
  return selected.id === "builtin"
    ? devices.find((entry) =>
        /macbook.*microphone|built[ -]?in.*mic|internal.*mic|microphone array/i.test(
          entry.label,
        ),
      )
    : devices.find((entry) => entry.deviceId === selected.id);
}
export async function allowVoxMicrophones() {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: AUDIO,
    video: false,
  });
  stream.getTracks().forEach((track) => track.stop());
}

/** Pin an explicit input before WebRTC starts. A missing selection is an error,
 * never permission to fall back to a Bluetooth/system-default microphone. */
export async function captureVoxMicrophone(
  isCurrent: () => boolean,
): Promise<MediaStream> {
  const selected = loadVoxMicrophone();
  if (selected.id === "system")
    return navigator.mediaDevices.getUserMedia({ audio: AUDIO, video: false });
  let devices = await listVoxMicrophones();
  let device = findVoxMicrophone(selected, devices);
  // Before the first permission grant browsers may hide device names. The
  // temporary permission stream is stopped before any peer/helper is created.
  if (!isCurrent()) throw new DOMException("cancelled", "AbortError");
  if (!device && (!devices.length || devices.some((entry) => !entry.label))) {
    await allowVoxMicrophones();
    if (!isCurrent()) throw new DOMException("cancelled", "AbortError");
    devices = await listVoxMicrophones();
    device = findVoxMicrophone(selected, devices);
  }
  if (!device)
    throw new VoxMicrophoneError(
      "the selected microphone is unavailable. choose an input in settings → vox and try again.",
    );
  if (!isCurrent()) throw new DOMException("cancelled", "AbortError");
  return navigator.mediaDevices
    .getUserMedia({
      audio: { ...AUDIO, deviceId: { exact: device.deviceId } },
      video: false,
    })
    .catch((error: unknown) => {
      if (
        error instanceof DOMException &&
        ["NotFoundError", "OverconstrainedError"].includes(error.name)
      )
        throw new VoxMicrophoneError(
          "the selected microphone is unavailable. choose an input in settings → vox and try again.",
        );
      throw error;
    });
}
