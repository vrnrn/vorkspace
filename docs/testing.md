# Verification

Run checks from the repository root with current stable Rust and Node.js 24+:

```sh
npm ci
npm run check
npm run assets
cargo build --locked -p vorkspace-gpui --profile native-dev
bash scripts/desktop/bundle-native-macos.sh native-dev target "$(node -p 'JSON.parse(require("fs").readFileSync("package.json")).version')" dev-data
```

`npm run check` covers host, Vox, build tooling, Rust formatting, Clippy, and every Rust test. Rust tests run under `cargo-nextest` (`cargo install --locked cargo-nextest`), where each test owns a process, so GPUI global state stays isolated while the whole suite runs in parallel. `npm run check:fast` runs tooling, Vox, formatting, Clippy, and core unit tests for fast iteration without the GPUI suite. The core suite exercises Copilot streaming/authentication against a local peer and Vox start, resume, duplicate tool calls, cancellation, and transcript flush against a disposable Codex protocol peer. Browser audio tests cover pinned devices, late permission grants, playback, and media disposal. Asset generation validates editor analysis, highlighting, completion, search, and formatting in a JavaScript interpreter.

For the real SSH transport, build a host package and run the disposable loopback test:

```sh
npm run host:package
python3 scripts/checks/test-remote-ssh.py
```

It needs local OpenSSH `sshd`, creates temporary keys/configuration, and exercises native askpass, host persistence, and reconnection. It does not alter normal SSH configuration.

GPUI tests marked ignored require their named platform resource or an explicit benchmark run. Inspect them with `cargo test -p vorkspace-gpui --profile native-dev -- --list`. A frame benchmark measures the specified CPU path; it does not prove GPU presentation rate.

Every Cargo profile compiles QuickJS with full optimization. Editor, diagram, and visual deadlines assume the shipped interpreter, and an unoptimized one is about five times slower, which fails work that a release build finishes in budget.

Interactive validation should use a temporary `VORKSPACE_NATIVE_DATA_DIR`: open fixtures, edit/save/reopen, exercise approvals and Stop, and quit during active work. Physical microphone capture, provider authentication, and installing signed updates require their real devices or credentials. A compile, mock provider, or signature inspection alone does not prove those flows.

## Installed application updates

Prepare update-enabled candidates as described in [updates](updates.md). Use disposable copies of the published old application and its data, then run its actual signature verification, installation and relaunch against the candidate. Confirm that a damaged archive is rejected and compare database rows, drafts and settings after restart. Exercise save and reopen in the new application, followed by a signed Sparkle update between native versions. Keep test feeds and private keys outside the repository; do not publish a release to test locally.

Composer regression tests cover existing-row restore, UTF-16 selections, opaque attachment and MCP fields, changed MCP server identity, input during restore, quit before restore completes, revision tombstones, and conflicting writers with durable recovery and explicit resolution.

Release-helper tests run with `npm run test:tooling`. Set `VORKSPACE_TEST_SPARKLE` to the downloaded SDK directory to include real Sparkle archive signing, signature rejection and dual-format package verification on macOS. Those tests use temporary keys and app bundles.

## Synthetic measurements

`node scripts/benchmarks/benchmark-host-startup.mjs --sessions 200 --chars 262144 --runs 7` measures startup against generated history in temporary data. Add `--baseline <commit>` to compare a historical JavaScript host: the benchmark exports that commit's complete host source before bundling it. Each sample starts a fresh Node process, and baseline/current order alternates. Component results do not establish end-to-end UI latency or GPU frame rate.
