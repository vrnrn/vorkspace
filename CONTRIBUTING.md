# Contributing

vorkspace is a fork of [MonoCode](https://github.com/hardbeat920/monocode) by Nick (hardbeat920). Small, focused bug reports and fixes are welcome in [this repository](https://github.com/vrnrn/vorkspace).

Please don’t open PRs that add a new provider right now. The existing harnesses still need to agree on a few patterns, and a new adapter would copy whatever is there today. See [New providers](#new-providers).

## Get it running

You need Node.js 20+, a current stable Rust toolchain, and at least one provider CLI installed and logged in. See [provider setup](docs/providers.md) for installation and login instructions.

Published desktop releases target Apple Silicon macOS. Source build instructions are also available for Linux and Windows. See [building from source](docs/building.md) for platform prerequisites and packaging commands.

```bash
npm install
npm run tauri dev
```

## Where things live

- `src/app/` - application composition, startup behavior, window shell, and release/update UI
- `src/features/` - product behavior grouped by feature, with UI, model, data access, hooks, and tests kept together
- `src/integrations/harness/` - the provider-independent harness core and one folder per provider adapter
- `src/platform/tauri/` - browser-to-Tauri adapters for filesystem, PTY, clipboard, and platform behavior
- `src/shared/` - reusable UI, hooks, and small utilities that contain no feature behavior
- `src-tauri/src/` - the Rust side: PTYs, filesystem and git, session storage, native window
- `host/` - remote host runtime, transport, persistence, process ownership, and tests
- `tests/browser/` - browser fixtures and cross-feature interaction tests
- `scripts/` - small maintenance, development, packaging, and release commands; focused script tests live alongside them

See [docs/architecture.md](docs/architecture.md) ([ELI5](docs/eli5/architecture.md)) for a full system breakdown.

`src/integrations/harness/` is the most useful place to start if you want to fix something real. Each folder under `providers/` has an adapter (`claudeAdapter.ts`) that implements the shared `HarnessAdapter` lifecycle from `core/registry.ts`, and a protocol module (`claudeProtocol.ts`) that translates the CLI’s output into vorkspace’s own event types. The protocol modules are pure functions with unit tests beside them, so you can fix a Codex parsing bug with only Claude Code installed. That’s for the providers we already ship - please don’t add a new one yet.

## Before you push

```bash
npm run check
```

`npm run check` runs the frontend unit suite and type check, then Rust formatting, Clippy, and tests. Host tests, script tests, and browser tests are separate checks; CI coverage can also vary by workflow.

```bash
npm run test:host
node --test scripts/*.test.*
```

`npm run check:web` and `npm run check:rust` run the two halves separately when you only touched one side.

To measure remote-host startup with the script's default isolated fixture (200 sessions, 262,144 characters each, seven runs; not application data):

```sh
node scripts/benchmark-host-startup.mjs
```

Pass `--baseline` with a Git ref to compare that revision with the current source.

For transcript layout and scrolling changes, also run the real-browser tests:

```bash
npx playwright install chromium webkit
npm run test:browser
```

These render the actual transcript and stylesheet in Chromium and WebKit, checking scroll geometry that the DOM-mocked unit tests cannot exercise. They start an isolated Vite server on port 1435 and need no provider or Tauri app.

## New providers

I’m pausing new harnesses until the current ones share the same patterns - session lifecycle, catalog probes, usage, approvals, and how slash commands and skills are wired. A PR that adds another provider will be closed for now, even if the work is good. Fixes, tests, and protocol bugs on Claude Code, Codex, Antigravity, Pi, and omp are still the best kind of contribution.

When the pause lifts, this section goes away.

## Pull requests

Keep a PR to one thing, and say what changed and why. The [PR template](.github/pull_request_template.md) covers the rest. If it changes the UI, a before/after screenshot helps a lot.

For anything that moves product direction - a new surface, new provider behavior, a refactor that changes the shape of the app - open an issue first. That’s not gatekeeping, I’d just rather you hear “I’m already halfway through that” before you write it than after. New providers are the exception: don’t send the adapter, even from an issue, until the pause above is gone.

I might close a PR, ask you to shrink it, or end up implementing the idea differently. That’s a call about scope and timing, not about you or the quality of your work.

Be kind: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Security reports: [SECURITY.md](SECURITY.md).
