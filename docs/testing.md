# Verification

Run checks from the repository root with current stable Rust and Node.js 24+:

```sh
npm ci
npm run check
npm run assets
cargo build --locked -p vorkspace-gpui -p vorkspace-helper --profile native-dev
bash scripts/desktop/bundle-native-macos.sh native-dev target "$(node -p 'JSON.parse(require("fs").readFileSync("package.json")).version')" dev-data
```

`npm run check` covers Vox, build tooling, Rust formatting, Clippy, and every Rust test, including the vendored `block` crate. Rust tests run under `cargo-nextest` (`cargo install --locked cargo-nextest`), where each test owns a process, so GPUI global state stays isolated while the whole suite runs in parallel. `npm run check:fast` runs tooling, Vox, formatting, Clippy, and core unit tests for fast iteration without the GPUI suite. The core suite exercises Copilot streaming/authentication against a local peer and Vox start, resume, duplicate tool calls, cancellation, and transcript flush against a disposable Codex protocol peer. Browser audio tests cover pinned devices, late permission grants, playback, and media disposal. Asset generation validates editor analysis, highlighting, completion, search, and formatting in a JavaScript interpreter.

Tooling checks also guard the repository itself. `module-cycles.test.mjs` keeps top-level modules depending in one direction, and `docs.test.mjs` fails if a document names an `npm run` script or `-p` crate that doesn't exist, or if a guide loses its plain-English companion.

Store fixtures use `Store::test` with the desktop schema. A caller-owned temporary path supports close/reopen checks; the `:memory:` fixture marker creates an owned temporary directory for the database and settings sidecar. The helper supplies empty desktop preferences, so fixtures do not inherit personal settings. Screenshot fixtures and the native benchmark use the same helper. Tests of database validation and historical formats also exercise the production read and open entry points directly against temporary files.

The clipboard lifecycle tests start `vorkspace-helper` with private sockets, so they stay separate from a running app. They hold ⌘⇧C briefly if the shortcut is available and report it as taken otherwise.

`ui_parity_capture` is an ignored test that renders desktop surfaces in dark and light through the headless Metal renderer into `VORKSPACE_UI_PARITY_DIR`, so captures from two trees can be compared byte for byte. The capture isn't fully isolated from the machine: a live notice, such as a harness update, can appear in it.

GPUI tests marked ignored require their named platform resource or an explicit benchmark run. Inspect them with `cargo test -p vorkspace-gpui --profile native-dev -- --list`. A frame benchmark measures the specified CPU path; it does not prove GPU presentation rate.

`npm run benchmark:native` drives the real shell offscreen, in the `perf` Cargo profile, against 300 generated sessions and a 4,000-row history in temporary data. `scene_*` metrics are main-thread CPU time to build a frame, not GPU time: they cover redraw, composer input, and appending to settled history, which is not the live streaming path. `metal_render_readback` is the only metric that reaches Metal, and it includes copying pixels back. Each result is a median over fresh processes. Save one outside the repository with `-- --save <path>` and check a change with `-- --compare <path>`.

Every Cargo profile compiles QuickJS with full optimization. Editor, diagram, and visual deadlines assume the shipped interpreter, and an unoptimized one is about five times slower, which fails work that a release build finishes in budget.

Interactive validation should use a temporary `VORKSPACE_NATIVE_DATA_DIR`: open fixtures, edit/save/reopen, exercise approvals and Stop, and quit during active work. Physical microphone capture, provider authentication, and installing signed updates require their real devices or credentials. A compile, mock provider, or signature inspection alone does not prove those flows.

## Installed application updates

Prepare update-enabled candidates as described in [updates](updates.md). Use disposable copies of the published old application and its data, then run its actual signature verification, installation and relaunch against the candidate. Confirm that a damaged archive is rejected and compare database rows, drafts and settings after restart. Exercise save and reopen in the new application, followed by a signed Sparkle update between native versions. Keep test feeds and private keys outside the repository; do not publish a release to test locally.

Composer regression tests cover existing-row restore, UTF-16 selections, opaque attachment and MCP fields, changed MCP server identity, input during restore, quit before restore completes, revision tombstones, and conflicting writers with durable recovery and explicit resolution.

Release-helper tests run with `npm run test:tooling`. Set `VORKSPACE_TEST_SPARKLE` to the downloaded SDK directory to include real Sparkle archive signing, signature rejection and dual-format package verification on macOS. Those tests use temporary keys and app bundles.
