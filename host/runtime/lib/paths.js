import { IS_WIN } from "../platform/platform.js";
function windowsPath(path) {
  return (
    /^[A-Za-z]:[\\/]/.test(path) ||
    path.startsWith("\\\\") ||
    path.startsWith("//")
  );
}
export function slash(path) {
  return windowsPath(path) || (IS_WIN && !path.startsWith("/"))
    ? path.replace(/\\/g, "/")
    : path;
}
function trimSlash(path) {
  return slash(path).replace(/\/+$/, "") || "/";
}
/** Stable comparison key for Windows paths without changing their display case. */
export function pathKey(path) {
  const normalized = trimSlash(path);
  return /^[A-Za-z]:(?:\/|$)/.test(normalized) || normalized.startsWith("//")
    ? normalized.toLowerCase()
    : normalized;
}
/** Path relative to cwd when it lives under the project, otherwise unchanged. */
export function displayPath(path, cwd) {
  const normalized = trimSlash(path);
  const base = cwd ? trimSlash(cwd) : undefined;
  if (base && base !== "~") {
    const key = pathKey(normalized);
    const baseKey = pathKey(base);
    if (key === baseKey) {
      return normalized.split("/").filter(Boolean).pop() || normalized;
    }
    const prefix = `${base}/`;
    if (key.startsWith(`${baseKey}/`)) {
      return normalized.slice(prefix.length);
    }
  }
  return normalized;
}
