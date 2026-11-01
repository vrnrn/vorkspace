# Building vorkspace

The root Cargo workspace owns the desktop and core. Use current stable Rust and Node.js 24+. macOS builds require Xcode with macOS platform components. Rust tests run through [`cargo-nextest`](https://nexte.st), which `cargo install --locked cargo-nextest` provides.

## Development and release

```sh
npm run dev -- --cwd /path/to/project
npm run build
```

Development defaults to the `release` Cargo profile and normal desktop data. Type `r` then Enter in its terminal to rebuild/restart; a failed build leaves the running app open. The runner opts into stdin-driven normal shutdown in either profile. On macOS, local release builds package `target/release/vorkspace.app` and install it at `/Applications/vorkspace.app`. If installation fails, the runner opens the freshly built bundle directly. Set `VORKSPACE_NO_INSTALL=1` to keep the generated bundle without installing it; CI also skips installation.

For the development profile with isolated data, use `npm run dev -- --native-dev --dev-data`; its macOS bundle is `target/native-dev/vorkspace.app`. The profile and data selection are independent: omit `--dev-data` or pass `--vorkspace-data` to use normal desktop data.

The app provides **Restart vorkspace** in its application menu and Command Palette (`⌘⌥R`). When developing from inside the installed app, run `npm run build`, confirm installation succeeded, then use Restart to load it. Restart prompts about unsaved file edits and uses the normal shutdown hooks for drafts, workspace state, voice, and provider processes. Its relaunch helper waits for the old process to exit before opening the successor. On macOS it prefers `/Applications/vorkspace.app`, so use the runner's `r` for a separate development bundle. Restart does not compile code; `cargo build` alone does not update the installed app bundle.

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

`npm run check` runs tooling, voice, host, formatting, Clippy, and workspace tests. `npm run test:rust` runs every Rust test in parallel, one process per test. `npm run host:package` creates a self-contained remote host archive including Node.

See [verification](testing.md), [data selection](data-storage.md), and [signed update packaging](updates.md).
