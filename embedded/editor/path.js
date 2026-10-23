export const basename = (path) =>
  path.replace(/\\/g, "/").replace(/\/+$/, "").split("/").pop() || "/";
