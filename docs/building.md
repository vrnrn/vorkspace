# Building vorkspace

The root Cargo workspace owns the desktop, its clipboard helper, and the shared crates. See the [repository map](repository.md) for what each crate owns.

## Requirements

- Current stable Rust. The repository does not pin a toolchain.
- Node.js 24 or newer.
- Xcode with macOS platform components, for Metal and for `xcrun swiftc`, which compiles the clipboard panel.
- [`cargo-nextest`](https://nexte.st) to run Rust tests: `cargo install --locked cargo-nextest`.

`npm ci` installs the build and test tools. The app itself needs no JavaScript dependencies to compile, so you can skip that step when you only want to run it.

## Development

```sh
npm run dev -- --cwd /path/to/project
```

This builds the app and helper in the `native-dev` Cargo profile, which rebuilds in seconds, packages `target/native-dev/vorkspace.app`, opens it with normal desktop data, and leaves `/Applications` alone. Type `r` then Enter in its terminal to rebuild and restart; a failed build leaves the running app open. The runner opts into stdin-driven normal shutdown in either profile.

| Option | Effect |
| --- | --- |
| `--release` | Use the release profile. This also installs to `/Applications/vorkspace.app`, unless `VORKSPACE_NO_INSTALL=1` is set. |
| `--profile NAME` | Use another Cargo profile, such as `dev` or `perf`. |
| `--dev-data` | Use isolated development data. `VORKSPACE_NATIVE_DATA_DIR` chooses its directory. |
| `--vorkspace-data` | Use normal desktop data, which is the default. |
| `--vorkspace-db PATH` | Use a specific database. See [data selection](data-storage.md). |
| `--cwd PATH` | Open this project. |
| `--check-data` | Check the selected database read-only and exit. |
| `--fixture`, `--chat-fixture`, `--visual-fixture` | Seed sample conversations. These need `--dev-data`. |

The profile and data selection are independent. Everything after `--` goes to the app unchanged.

### The clipboard helper

History, ⌘⇧C and the menu bar icon belong to `vorkspace-clipboard`, a separate process nested in the bundle at `Contents/Helpers/vorkspace Clipboard.app`. While **Keep recent copies** is on, it keeps running after the development app exits, by design:

- Rebuilding replaces it on the next launch, which clears its history.
- Each bundle path gets its own helper, so a development build and the installed app can run side by side. You'll see two menu bar icons, and only the first gets ⌘⇧C. The other reports that in Settings.
- To stop it, use Quit in the clipboard menu or turn off **Keep recent copies**.

`cargo run` builds only the app, and the app looks for the helper beside its own binary. To include it, build both in the same profile:

```sh
cargo build --locked -p vorkspace-clipboard --profile native-dev
cargo run --locked -p vorkspace-gpui --profile native-dev
```

## Release builds

```sh
npm run build
```

This compiles the app and helper with the release profile, packages `target/release/vorkspace.app`, and installs it to `/Applications/vorkspace.app` when that's safe. `VORKSPACE_NO_INSTALL=1` skips installing. To package a build by hand, use the commands in [verification](testing.md).

The app provides **Restart vorkspace** in its application menu and Command Palette (`⌘⌥R`). When developing from inside the installed app, run `npm run build`, confirm installation succeeded, then use Restart to load it. Restart relaunches the bundle that is running, so a `native-dev` build stays on `target/native-dev/vorkspace.app` even when an older copy is installed. Restart prompts about unsaved file edits and uses the normal shutdown hooks for drafts, workspace state, voice, and provider processes. Its relaunch helper waits for the old process to exit before opening the successor. On macOS it prefers `/Applications/vorkspace.app`, so use the runner's `r` for a separate development bundle. Restart does not compile code; `cargo build` alone does not update the installed app bundle.

`CARGO_TARGET_DIR` and `CARGO_BUILD_TARGET` override output placement. `CARGO_BUILD_JOBS` controls build concurrency. `npm run build` packages and installs locally but does not launch the app or publish a release.

The manual **Mac release** workflow runs from `main`. It selects an unused stable version at or above the checked-in version, stamps package and Cargo manifests in the runner only, and assigns the native build number from the workflow run and attempt. Draft retries retain their own reserved version; published releases are immutable. The workflow generates versioned notes before compilation, runs checks, prepares signed update formats and a disk image, verifies metadata, and then publishes. Starting this workflow is an explicit release action; ordinary pushes and pull requests do not trigger it.

Release preparation writes ignored `build/release-notes.md` and `build/native-release.json` artifacts. `VORKSPACE_RELEASE_BUILD=1` requires notes matching the Cargo package version; normal local builds include matching generated notes when available. The app reads these bundled notes offline and records version observations so an installed update notice appears once per version change. Opening What's new for an unbundled version loads its matching published GitHub notes in the background, with a 10-second timeout and 256 KiB response limit. See [update signing](updates.md) for release secrets.

## Generated assets and tests

```sh
npm ci
npm run assets
npm run check
```

The editor and formatter bundles are committed under `crates/vorkspace-gpui/assets/`; the Vox audio bundle lives under `crates/vorkspace-core/assets/`. Their maintained sources live in `embedded/editor` and `embedded/vox`. These files are embedded, so Cargo can compile without installing JavaScript dependencies. `npm run assets` installs the separate editor generator dependencies and rebuilds these bundles. Root npm dependencies are build/test tools only.

The GPUI fork and a patched `block` crate live in `vendor/` and are edited directly. `cargo clippy` with `-D warnings` doesn't cover them, so check that a rebuild stays free of warnings after changing one. `npm run test:rust` runs the `block` tests separately because that crate sits outside the workspace.

In a new linked worktree, run `npm run worktree:seed` before the first build. To have Vorkspace do this for its own worktrees, set the project's setup command to `npm run worktree:seed` under **Settings → Worktrees**. It clones the main checkout's Cargo target directory, honoring `CARGO_TARGET_DIR`, with APFS copy-on-write, so the clone is instant and takes no extra disk, and only workspace and vendored crates rebuild.

`npm run check` runs tooling, voice, host, formatting, Clippy, and workspace tests. `npm run check:fast` skips the host and GPUI suites. `npm run test:rust` runs every Rust test in parallel, one process per test. `npm run host:package` creates a self-contained remote host archive including Node.

See [verification](testing.md), [data selection](data-storage.md), and [signed update packaging](updates.md).
