#!/bin/bash
# Starts a new worktree from the main checkout's build output. APFS clones are
# instant and share disk blocks; Cargo reuses every crate that lives outside the
# checkout, which leaves only the workspace and vendored crates to rebuild.
set -eu
target_directory() {
  # Cargo resolves a relative CARGO_TARGET_DIR against each checkout.
  (cd "$1" && cargo metadata --no-deps --offline --format-version 1) |
    node -pe 'JSON.parse(require("fs").readFileSync(0)).target_directory'
}
root=$(git rev-parse --show-toplevel)
main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
if [ "$root" = "$main" ]; then
  echo "Run this from a linked worktree, not the main checkout." >&2
  exit 1
fi
source=$(target_directory "$main")
destination=$(target_directory "$root")
if [ "$source" = "$destination" ]; then
  echo "Both checkouts already share $source; nothing to clone."
  exit 0
fi
if [ -e "$destination" ]; then
  echo "$destination already exists; leaving it alone." >&2
  exit 1
fi
if [ ! -d "$source" ]; then
  echo "$source does not exist yet; build there first." >&2
  exit 1
fi
mkdir -p "$(dirname "$destination")"
cp -cRp "$source" "$destination"
echo "Cloned $source"
