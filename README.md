<p align="center">
  <img src="docs/assets/vorkspace-banner.png" alt="vorkspace" width="760" />
</p>

# vorkspace

A native GPUI workspace for working with coding agents, projects, conversations, files, terminals, Bots, and Channels.

The desktop is Rust. Provider CLIs run as supervised child processes. A small JavaScript layer supports remote hosts, editor analysis and formatting, and WebRTC audio for Vox. Vox control and the desktop Copilot gateway run in Rust.

## Run locally

You need a current stable Rust toolchain, Node.js 24 or newer, and Xcode with macOS components. Install and sign in to at least one supported [provider CLI](docs/providers.md).

```sh
npm run dev -- --cwd /path/to/project
```

This builds the app and its clipboard helper in the fast-rebuilding `native-dev` profile, runs them from `target/native-dev/`, and leaves `/Applications` alone. It opens your normal desktop data, so add `--dev-data` for an isolated launch. Type `r` then Enter in the terminal to rebuild and restart.

To test, install `cargo-nextest` once, then run the full check:

```sh
cargo install --locked cargo-nextest
npm ci
npm run check
```

`npm run build` makes the release bundle at `target/release/vorkspace.app` and installs it to `/Applications/vorkspace.app`. See [building](docs/building.md) for every option.

## Features and architecture

- Native conversations, approvals, Stop, drafts, attachments, provider accounts, and saved session history.
- Files, editor syntax and formatting, Git changes, worktrees, search, and terminal docks.
- Persistent Bots, scheduled habits, Channels, notes, and agent-generated visuals.
- SSH connections to a Node host that owns remote processes and durable sessions.
- Optional Vox calls controlled by Rust with a private WebRTC audio page; microphone access starts only with a call.
- macOS app packaging with optional Sparkle signing and update configuration.

vorkspace is a macOS app. Remote hosts can run on macOS or Linux.

See the [repository map](docs/repository.md) for source ownership and the [documentation index](docs/README.md) for all guides.

Read [building](docs/building.md), [architecture](docs/architecture.md), [data continuity](docs/data-storage.md), [remote access](docs/remote-access.md), [Vox](docs/vox.md), and [release packaging](docs/updates.md). Each technical document has a matching plain-English version under `docs/eli5/`.

## License

MIT for application code; vendored GPUI code is Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for attribution and embedded dependency notices.
