# Installing Vorkspace (ELI5)

Getting started with Vorkspace takes just two steps: install at least one AI coding tool, then put Vorkspace in your Applications folder.

For the full platform guide and Linux commands, see [Installation](../install.md).

---

## 1. Install on a Mac

1. First, make sure you have at least one AI agent installed and signed in (like Claude Code or Codex—see [Providers](providers.md)).
2. Download `vorkspace_X.Y.Z_aarch64.dmg` from the releases page and drag **`vorkspace.app`** into your **`~/Applications`** folder.
3. Because Vorkspace isn't signed with a paid Apple corporate certificate, macOS might ask if you trust it the very first time you open it. Go to **System Settings → Privacy & Security → Open Anyway**. After that, future updates are verified with Vorkspace's own security key right inside the app.

---

## 2. Switching from MonoCode

Vorkspace grew out of MonoCode and uses the same saved-data folder (`com.monocode.desktop`) on your Mac:
- All your existing chats, Bots, notes, and settings show up automatically!
- Just don't run both MonoCode and Vorkspace at the exact same time.
- Run the migration and backup script (`bash scripts/migrate-monocode-to-vorkspace.sh --apply`) to save a safety copy of your data and set up your `bots/` and `codex-bots/` folders.

---

## 3. How Drafts and Pasted Images Are Saved

- **Unsent Typing**: If you type half a message and close the app, Vorkspace remembers it when you come back.
- **Pasted Screenshots**: When you paste an image into a chat, Vorkspace saves a private copy in its data folder. If you remove a pasted image and never send it, Vorkspace waits an hour and then quietly cleans up the unused file on a later startup.

---

## 4. Installing on Linux

If you built Vorkspace on Linux (Ubuntu/Debian `.deb`, Fedora/RHEL 10 `.rpm`, or `.AppImage`), install it with `apt` or `dnf` so your system automatically grabs the WebKit display library (`webkit2gtk4.1`) that Vorkspace needs to draw its window.
