#!/bin/bash
# Starts a new worktree from the main checkout's build output. APFS clones are
# instant and share disk blocks; Cargo reuses every crate that lives outside the
# checkout, which leaves only the workspace and vendored crates to rebuild.
set -eu
root=$(git rev-parse --show-toplevel)
main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
if [ "$root" = "$main" ]; then
  echo "Run this from a linked worktree, not the main checkout." >&2
  exit 1
fi
if [ -e "$root/target" ]; then
  echo "$root/target already exists; leaving it alone." >&2
  exit 1
fi
if [ ! -d "$main/target" ]; then
  echo "$main/target does not exist yet; build there first." >&2
  exit 1
fi
cp -cRp "$main/target" "$root/target"
echo "Cloned $main/target"
