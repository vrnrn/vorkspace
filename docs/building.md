# Building vorkspace

The root Cargo workspace owns the desktop app, its background helper, and the shared crates. See the [repository map](repository.md) for what each crate owns.

## Requirements

- macOS 26 or newer. The native deployment target and both app bundles require 26.0.
- Rust through `rustup`. `rust-toolchain.toml` pins the compiler and installs Rustfmt, Clippy, and LLVM tools. LLVM tools includes the matching library needed by `rust-objcopy` on macOS.
- Zig 0.17.x to compile the bundled libghostty-vt terminal engine. Cargo builds and links it statically; the installed app does not require Zig.
- Node.js 24 or newer.
- Xcode with macOS 26 or newer platform components, for Metal and for `xcrun swiftc`, which compiles the clipboard panel and native speech bridge.
- [`cargo-nextest`](https://nexte.st) to run Rust tests: `cargo install --locked cargo-nextest`.
- A checkout of [fluidither](https://github.com/vrnrn/fluidither) beside this repository at `../fluidither-gpui-pre`, on a branch with the `pre` feature. `vorkspace-ui` builds its GPUI crate against this workspace's GPUI fork and uses it for dither, springs and Obel Mono (see `DESIGN.md`).

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

### The background helper

The `vorkspace-helper` process handles clipboard history, ⌘⇧C, dictation and meeting capture. The bundle includes it at `Contents/Helpers/vorkspace helper.app`. It stays available after the development app exits while **Keep recent copies** or **Dictation** is on.

- Rebuilding replaces it on the next launch, which clears its history.
- Each bundle path gets its own helper, so a development build and the installed app can run side by side. You'll see two menu bar icons, and only the first gets ⌘⇧C. The other reports that in Settings.
- Quit in the clipboard menu stops the helper. When no note is recording, turning off both **Keep recent copies** and **Dictation** also stops it. See the [dictation guide](dictation.md) for permissions and custom words.

`cargo run` builds only the app, and the app looks for the helper beside its own binary. To include it, build both in the same profile:

```sh
cargo build --locked -p vorkspace-helper --profile native-dev
cargo run --locked -p vorkspace-gpui --profile native-dev
```

## Release builds

```sh
npm run build
```

This compiles the app and helper with the release profile, packages `target/release/vorkspace.app`, and installs it to `/Applications/vorkspace.app` when that's safe. `VORKSPACE_NO_INSTALL=1` skips installing. To package a build by hand, use the commands in [verification](testing.md).

The app provides **Restart vorkspace** in its application menu and Command Palette (`⌘⌥R`). When developing from inside the installed app, run `npm run build`, confirm installation succeeded, then use Restart to load it. Restart relaunches the bundle that is running, so a `native-dev` build stays on `target/native-dev/vorkspace.app` even when an older copy is installed. Restart prompts about unsaved file edits and uses the normal shutdown hooks for drafts, workspace state, voice, and provider processes. Its relaunch helper waits for the old process to exit before opening the successor. Restart does not compile code; use the runner's `r` to rebuild a development bundle. `cargo build` alone does not update the installed app bundle.

`CARGO_TARGET_DIR` and `CARGO_BUILD_TARGET` override output placement. `CARGO_BUILD_JOBS` controls build concurrency. `npm run build` packages and installs locally but does not launch the app or publish a release.

Cargo forces the macOS deployment target to 26.0. The Swift bridge, Ghostty terminal library, and app/helper metadata use the same minimum. Packaging checks the actual Mach-O deployment commands in both executables before copying or signing them, and rejects a mismatched minimum in any architecture slice. Prebuilt third-party frameworks may support older systems; they do not lower the app's minimum.

The manual **Mac release** workflow runs from `main`. It selects an unused stable version at or above the checked-in version, stamps package and Cargo manifests in the runner only, and assigns the native build number from the workflow run and attempt. Draft retries retain their own reserved version; published releases are immutable. The workflow generates versioned notes before compilation, runs checks, prepares signed update formats and a disk image, verifies metadata, and then publishes. Starting this workflow is an explicit release action; ordinary pushes and pull requests do not trigger it.

Release preparation writes ignored `build/release-notes.md` and `build/native-release.json` artifacts. `VORKSPACE_RELEASE_BUILD=1` requires notes matching the Cargo package version; normal local builds include matching generated notes when available. The app reads these bundled notes offline and records version observations so an installed update notice appears once per version change. Opening What's new for an unbundled version loads its matching published GitHub notes in the background, with a 10-second timeout and 256 KiB response limit. See [update signing](updates.md) for release secrets.

## Generated assets and tests

```sh
npm ci
npm run assets
npm run check
```

The editor and formatter bundles are committed under `crates/vorkspace-gpui/assets/`; the Vox audio bundle lives under `crates/vorkspace-core/assets/`. Their maintained sources live in `embedded/editor` and `embedded/vox`. These files are embedded, so Cargo can compile without installing JavaScript dependencies. `npm run assets` installs the separate editor generator dependencies and rebuilds these bundles. Root npm dependencies are build/test tools only.

The GPUI fork and a patched `block` crate live in `vendor/` and are edited directly. The fork adds a `dither` background (`Background::Dither`, a 4×4 Bayer pattern in the Metal quad shader) that `vorkspace_ui::fluid` owns. `cargo clippy` with `-D warnings` doesn't cover them, so check that a rebuild stays free of warnings after changing one. `npm run test:rust` runs the `block` tests separately because that crate sits outside the workspace.

## Build output and disk

Cargo keeps every build's output and never deletes superseded files, so a busy checkout grows by gigabytes a day unless something prunes it. The repository bounds it in four ways:

- **Light debug info.** The `dev` profile, used by plain `cargo build`, `cargo test` and editor tooling, keeps file-and-line tables for workspace crates and no debug info for dependencies. `native-dev` and `verify` carry none.
- **Pruning after builds.** `scripts/desktop/prune-target.mjs` keeps the two newest linked binaries and incremental caches per crate in each profile and removes the rest; libraries stay. `npm run dev` runs it after each successful build, `npm run test:rust` after its tests, and `npm run target:prune` on demand.
- **One output directory per checkout.** Do not point a task at its own `CARGO_TARGET_DIR`; that duplicates every dependency.
- **Warm worktrees.** Seed new worktrees as described below rather than building them cold.

Two optional machine-level tools complement these. A capped [sccache](https://github.com/mozilla/sccache) (`build.rustc-wrapper` in `~/.cargo/config.toml`, size in its own config) lets any checkout rebuild dependencies from cache. A weekly `cargo sweep --recursive --time 7` plus `--maxsize 20GB` over your projects folder removes output unused for a week. Neither is required to build.

In a new linked worktree, run `npm run worktree:seed` before the first build. To have Vorkspace do this for its own worktrees, set the project's setup command to `npm run worktree:seed` under **Settings → Worktrees**. It clones the main checkout's Cargo target directory, honoring `CARGO_TARGET_DIR`, with APFS copy-on-write, so the clone is instant and takes no extra disk, and only workspace and vendored crates rebuild.

`npm run check` runs tooling, voice, host, formatting, Clippy, and workspace tests. `npm run check:fast` skips the host and GPUI suites. Both use the `verify` Cargo profile: application code is unoptimized, dependencies retain basic optimization, and QuickJS retains its release-level optimization so interpreter deadlines remain meaningful. Debug assertions stay enabled. The interactive `native-dev` and shipped `release` profiles are separate. `npm run test:rust` runs every Rust test in parallel, one process per test. `npm run host:package` creates a self-contained remote host archive including Node.

Native CI and Mac release use the pinned compiler components and checksum-verified Zig 0.17.0. They share a dependency cache keyed by Rust, manifests, compiler flags, and the macOS SDK. Application source still builds and every check still runs on a cache hit. CI packages the `verify` build to verify bundle metadata and signing; release packaging uses `release`. A compiler, SDK, or dependency change can require a cold build.

See [verification](testing.md), [data selection](data-storage.md), and [signed update packaging](updates.md).
