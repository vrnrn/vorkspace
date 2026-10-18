<p align="center">
  <img src="public/vorkspace.png" alt="vorkspace" width="88" />
</p>

<h1 align="center">vorkspace</h1>

<p align="center">
  <strong>A GUI for your coding agents.</strong>
</p>

vorkspace is based on [MonoCode](https://github.com/hardbeat920/monocode), created by [Nick (hardbeat920)](https://github.com/hardbeat920). Thanks to Nick and the upstream contributors for the foundation.

Works with your subscriptions on Claude Code, Codex, Pi, omp, and Antigravity. If they’re installed and logged in, vorkspace can run them. Tabs are sessions. The composer is the input. vorkspace does not sell tokens.

## Install

Install and sign in to at least one supported agent first. See [provider setup](docs/providers.md) for instructions.

Download the Apple Silicon DMG from [vorkspace releases](https://github.com/vrnrn/vorkspace/releases/latest). Release files are named `vorkspace_X.Y.Z_aarch64.dmg`.

Open the DMG and copy `vorkspace.app` to `~/Applications`. The fork publishes Apple Silicon macOS builds. For source builds on other platforms, see [build setup](docs/building.md).

## Get started

Open a project folder, choose an agent, and send a message. Each tab is a session. Split panes to work with sessions side by side.

- Keep conversations organized by project.
- Use separate Git worktrees for parallel tasks.
- Edit files and review changes in the app.
- Start a message with `/operator` to let an agent open sessions and manage worktrees and notes.

Monos are experimental agents that work across your projects. They keep a memory and can run scheduled tasks you set up with them.

vorkspace is still early. If something breaks, [report a bug](https://github.com/vrnrn/vorkspace/issues).

## Guides

- [Architecture overview](docs/architecture.md) ([ELI5](docs/eli5/architecture.md))
- [Provider setup](docs/providers.md) ([ELI5](docs/eli5/providers.md))
- [Installation](docs/install.md) ([ELI5](docs/eli5/install.md))
- [Agent access with /operator](docs/agent-access.md) ([ELI5](docs/eli5/agent-access.md))
- [Vox voice controller](docs/vox.md) ([ELI5](docs/eli5/vox.md))
- [Jira Cloud inbox](docs/jira.md) ([ELI5](docs/eli5/jira.md))
- [Ticket context links](docs/ticket-context.md) ([ELI5](docs/eli5/ticket-context.md))
- [Remote sessions (experimental)](docs/remote-access.md) ([ELI5](docs/eli5/remote-access.md))
- [Building from source](docs/building.md) ([ELI5](docs/eli5/building.md))

## Build from source

You’ll need Node.js 20+ and a stable Rust toolchain. See [build setup](docs/building.md) for platform dependencies and packaging.

```bash
npm install
npm run tauri dev
```

Small, focused contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## License

The original MonoCode code is covered by its [MIT license](LICENSE). vorkspace changes are covered by the [vorkspace MIT license](LICENSE.vorkspace). Provider names and logos are trademarks of their owners. See [NOTICE](NOTICE).
