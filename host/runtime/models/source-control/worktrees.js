export function namedWorktreeBranch(fragment) {
  const clean = fragment
    .trim()
    .replace(/^(?:mc|vorkspace)\/+/, "")
    .replace(/^\/+|\/+$/g, "");
  return clean ? `mc/${clean}` : null;
}
