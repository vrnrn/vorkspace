# How Vorkspace Works (ELI5)

Think of **Vorkspace** like a **mission control room** for your AI coding helpers (like Claude Code, Codex, Antigravity, Pi, and omp).

Instead of talking to one helper at a time in a plain black terminal window, Vorkspace gives you a clean desk where several helpers can work side by side, each in their own folder, without bumping into each other.

For the full technical breakdown, see [Architecture overview](../architecture.md).

---

## 1. The Three Parts of the App

1. **The Screen You See (The Frontend — `src/`)**
   - Built with React and TypeScript.
   - It shows your chat tabs, code editor, Git changes, and floating helper windows (like the Quick Composer and floating Bot chat).
   - You can use either the classic full-window layout or an **Inset Glass Layout** with 11 color themes, smooth sliding project spaces, and project-scoped tabs for Inbox, Notes, and Automations.
   - When an AI helper types back really fast, Vorkspace only redraws the specific chat box that is talking—and colors big Git diffs on a background worker thread—so the rest of the screen stays buttery smooth.

2. **The Engine Under the Hood (The Rust Backend — `src-tauri/`)**
   - Built in Rust so it is fast and safe.
   - It starts and stops the AI command-line tools on your computer, watches over your files and Git branches, and saves your conversations into a local SQLite database (`monocode.db`).
   - If an AI tool prints a giant wall of output from a command, the Rust engine tucks the giant output away on disk and only loads it when you click to expand it.
   - It also lowers the priority of background AI tools so your computer never freezes while an agent runs tests, and shows a **High usage** badge in the footer if a tool or agent is hogging CPU or memory so you can stop just that process.

3. **The Remote Helper on Other Computers (`host/`)**
   - Sometimes your code lives on a big Linux server, a Mac mini, or a Windows desktop instead of your laptop.
   - Vorkspace can connect over SSH and install a tiny background helper (`vorkspace-host`) on that other computer.
   - Even if you close your laptop and walk away, the remote helper keeps your AI agents running and catches you up as soon as you reconnect.

---

## 2. How Agents Stay Safe and Organized

- **Separate Sandboxes (Git Worktrees)**: If you ask three agents to build three different features at the same time, Vorkspace gives each agent its own copy of the project folder (a Git worktree) so they don't overwrite each other's files.
- **Seamless Account Switching**: If your Claude Code or Codex account hits a usage limit mid-chat, you can pick another account right in the banner. Vorkspace carefully copies all the hidden memory and subagent files to the new account before moving the main conversation file last, so your agent picks up right where it left off.
- **Undo Checkpoints**: Before an agent touches your files on a turn, Vorkspace takes a quick snapshot so you can click **Undo** to roll back that turn's file edits.
- **Locked Preview Windows**: When an agent builds an HTML preview for you, Vorkspace shows it inside a locked glass box (`iframe` sandbox) that cannot touch your computer or your private files.
- **No Lost Messages**: Before sending your message to an agent, Vorkspace writes down a "receipt" on disk. If the app closes unexpectedly, it knows what was being sent and recovers your draft safely.
