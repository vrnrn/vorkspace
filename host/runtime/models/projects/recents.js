import { slash } from "../../lib/paths.js";
export function normalizeProjectPath(path) {
  return slash(path).replace(/\/+$/, "") || "/";
}
