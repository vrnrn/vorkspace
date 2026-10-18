import { readFlag } from "../../settings/model/storageFlags";

const KEY = "vorkspace.vox.silent";
const CHANGED = "vorkspace:vox-reply-mode";

export const loadVoxSilent = () => readFlag(KEY) ?? false;
export function saveVoxSilent(silent: boolean) {
  localStorage.setItem(KEY, silent ? "1" : "0");
  window.dispatchEvent(new Event(CHANGED));
}
export function subscribeVoxSilent(changed: () => void) {
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
