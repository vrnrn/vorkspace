<p align="center">
  <img src="docs/assets/vorkspace-banner.png" alt="vorkspace" width="760" />
</p>

# vorkspace

A native GPUI workspace for working with coding agents, projects, conversations, files, terminals, Bots, and Channels.

The desktop is Rust. Provider CLIs run as supervised child processes. A small JavaScript layer supports remote hosts, editor analysis and formatting, and WebRTC audio for Vox. Vox control and the desktop Copilot gateway run in Rust.

## Run locally

Install a current stable Rust toolchain, Node.js 24 or newer, and Xcode with macOS components. Install and sign in to at least one supported [provider CLI](docs/providers.md).

```sh
npm run dev -- --cwd /path/to/project
```

Development opens isolated data. No npm dependency install is needed to compile the committed native assets. Type `r` then Enter to rebuild and restart.

```sh
npm ci
npm test
npm run build
```

The macOS release bundle is `target/release/vorkspace.app`. Release builds use existing desktop data; use `--dev-data` for an isolated launch. The build does not install, publish, or update an application automatically.

## Features and architecture

- Native conversations, approvals, Stop, drafts, attachments, provider accounts, and saved session history.
- Files, editor syntax and formatting, Git changes, worktrees, search, and terminal docks.
- Persistent Bots, scheduled habits, Channels, notes, and agent-generated visuals.
- SSH connections to a Node host that owns remote processes and durable sessions.
- Optional Vox calls controlled by Rust with a private WebRTC audio page; microphone access starts only with a call.
- macOS app packaging with optional Sparkle signing and update configuration.

macOS is the supported validation target. Linux and Windows platform code is retained; those GUI builds need platform-specific validation.

See the [repository map](docs/repository.md) for source ownership and the [documentation index](docs/README.md) for all guides.

Read [building](docs/building.md), [architecture](docs/architecture.md), [data continuity](docs/data-storage.md), [remote access](docs/remote-access.md), [Vox](docs/vox.md), and [release packaging](docs/updates.md). Each technical document has a matching plain-English version under `docs/eli5/`.

## License

MIT for application code; vendored GPUI code is Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for attribution and embedded dependency notices.
