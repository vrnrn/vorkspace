# Agent Access with `/operator` (ELI5)

Normally, an AI agent in a chat tab only works inside its own conversation and its own project folder. It doesn't know how to click buttons inside Vorkspace itself.

When you start a message with **`/operator`** (or chat with a **Bot**), you hand that agent a special walkie-talkie (`vorkspace app`) that lets it control Vorkspace for you during that conversation.

For the full reference, see [Agent access with /operator](../agent-access.md).

---

## What Can an `/operator` Agent Do?

Once you type `/operator` in a thread, that agent can:
- **Start new agent tabs (`sessions.start`)**: For example, you can say *"/operator open two tabs—one to check the backend and one to check the UI"* and it will open and split those panes for you.
- **Create Git worktrees (`worktrees.create`)**: It can set up fresh branch folders so multiple agents can work in parallel without stepping on each other's code.
- **Check on other tabs (`sessions.list`, `sessions.read`)**: It can look at your other sessions in the project and read their recent replies.
- **Send follow-ups or drafts (`sessions.send`, `sessions.draft`)**: It can message an idle session or leave a draft message in its box for you to review before sending.
- **Organize and clean up (`folders.move`, `sessions.stop`, `sessions.archive`, `sessions.delete`)**: It can tidy sessions into sidebar folders or stop/archive finished ones.
- **Read and write your Notes (`notes.list`, `notes.read`, `notes.write`)**: It can look up your saved notes in Vorkspace or jot down a new note for you.

---

## How Vorkspace Keeps It Safe

- **Opt-In per Thread**: Only the thread where you typed `/operator` gets the walkie-talkie. Other chats cannot use it unless you give it to them too.
- **Only While Working**: The agent can only run Vorkspace commands while it is actively answering your turn.
- **Can't Delete Itself or Your Code**: Stopping or archiving a session only manages the chat—it never deletes your open files, terminals, or Git worktrees.
