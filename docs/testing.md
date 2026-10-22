# Verification

Run checks from the repository root with current stable Rust and Node.js 24+:

```sh
npm ci
npm run check
npm run assets
cargo build --locked -p vorkspace-gpui --profile native-dev
bash scripts/desktop/bundle-native-macos.sh native-dev target 0.13.8 dev-data
```

Use the workspace version instead of the example packaging version when it changes. `npm run check` covers host, Vox, build tooling, Rust formatting, Clippy, core tests, and serial GPUI tests. The core suite exercises Copilot streaming/authentication against a local peer and Vox start, resume, duplicate tool calls, cancellation, and transcript flush against a disposable Codex protocol peer. Browser audio tests cover pinned devices, late permission grants, playback, and media disposal. Asset generation validates editor analysis, highlighting, completion, search, and formatting in a JavaScript interpreter.

For the real SSH transport, build a host package and run the disposable loopback test:

```sh
npm run host:package
python3 scripts/checks/test-remote-ssh.py
```

It needs local OpenSSH `sshd`, creates temporary keys/configuration, and exercises native askpass, host persistence, and reconnection. It does not alter normal SSH configuration.

GPUI tests marked ignored require their named platform resource or an explicit benchmark run. Inspect them with `cargo test -p vorkspace-gpui --profile native-dev -- --list`. A frame benchmark measures the specified CPU path; it does not prove GPU presentation rate.

Interactive validation should use a temporary `VORKSPACE_NATIVE_DATA_DIR`: open fixtures, edit/save/reopen, exercise approvals and Stop, and quit during active work. Physical microphone capture, provider authentication, and installing signed updates require their real devices or credentials. A compile, mock provider, or signature inspection alone does not prove those flows.

## Synthetic measurements

`node scripts/benchmarks/benchmark-host-startup.mjs --sessions 200 --chars 262144 --runs 7` measures startup against generated history in temporary data. Add `--baseline <commit>` to compare a historical JavaScript host: the benchmark exports that commit's complete host source before bundling it. Each sample starts a fresh Node process, and baseline/current order alternates. Component results do not establish end-to-end UI latency or GPU frame rate.
