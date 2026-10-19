# Building Vorkspace from Source (ELI5)

If you want to tinker with Vorkspace's code or build the app yourself on your own Mac, Linux, or Windows computer, here is how it works in simple terms.

For the exact commands and package names, see [Build from source](../building.md).

---

## 1. What You Need First

Vorkspace is made of two halves:
- **Node.js (v20+)**: Builds the visual screen (buttons, tabs, editor) written in TypeScript and React.
- **Rust**: Builds the native desktop engine that talks to your operating system, files, and terminals.

You also need at least one AI agent CLI (like Claude Code or Codex) installed so Vorkspace has an agent to run.

---

## 2. Running in Development Mode

When you run:

```bash
npm install
npm run tauri dev
```

Vorkspace opens a special **vorkspace Dev** app (`com.vrnrn.vorkspace.development`).
- **Why is it separate?** So you can test changes and break things without touching your real everyday chats, notes, or settings in your normal `vorkspace.app`!

---

## 3. Packaging the App for Different Computers

- **Apple Silicon Macs**: Built automatically by GitHub Actions (or locally with `npm run build:mac`). Produces `vorkspace.app` and a `.dmg` installer. Updates are signed with a cryptographic key so your app can update itself safely in `~/Applications`.
- **Ubuntu / Debian Linux**: Run `npm run setup:linux:deb` once to install Linux window libraries, then `npm run build:linux` to create a `.deb` installer and an `.AppImage`.
- **Fedora / Enterprise Linux 10**: Run `npm run setup:linux:fedora` and `npm run build:fedora` to create an `.rpm` package.
- **Windows**: Run `npm run build:windows` to create a standard Windows installer (`.exe` via NSIS).

Each published Mac build includes its own **What's new** notes, so you can read them offline. The release process creates these notes from the merged changes and checks that the app includes the same text as the release page. Notes for other versions, and source builds without a bundled copy, load from that version's GitHub release when you open them. If the connection fails, you can try again.
