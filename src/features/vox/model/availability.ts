import { readFlag } from "../../settings/model/storageFlags";
const KEY = "vorkspace.vox.enabled";
const CHANGED = "vorkspace:vox-availability";
export const loadVoxEnabled = () => readFlag(KEY) ?? false;
export function saveVoxEnabled(enabled: boolean) {
  localStorage.setItem(KEY, enabled ? "1" : "0");
  window.dispatchEvent(new Event(CHANGED));
}
export function subscribeVoxEnabled(changed: () => void) {
  const stored = (event: StorageEvent) => {
    if (event.key === KEY || event.key === null) changed();
  };
  window.addEventListener(CHANGED, changed);
  window.addEventListener("storage", stored);
  return () => {
    window.removeEventListener(CHANGED, changed);
    window.removeEventListener("storage", stored);
  };
}
