# Agent Access with `/operator` (ELI5)

Normally, an AI agent in a chat tab only works inside its own conversation and its own project folder. It doesn't know how to click buttons inside Vorkspace itself.

When you start a message with **`/operator`** (or chat with a **Bot**), you hand that agent a special walkie-talkie (`vorkspace app`) that lets it control Vorkspace for you during that conversation.

For the full reference, see [Agent access with /operator](../agent-access.md).

---

## What Can an `/operator` Agent Do?

Once you type `/operator` in a thread, that agent can:

- **Start new agent tabs (`sessions.start`)**: For example, you can say *"/operator open two tabs—one to check the backend and one to check the UI"* and it will create new tabs for you.
- **Create Git worktrees (`worktrees.create`)**: It can set up fresh branch folders so multiple agents can work in parallel without stepping on each other's code.
- **Check on other tabs (`sessions.list`, `sessions.read`)**: It can look at your other sessions in the project and read their recent replies.
- **Send follow-ups or drafts (`sessions.send`, `sessions.draft`)**: It can message an idle session or leave a draft message in its box for you to review before sending.
- **Organize and clean up (`folders.move`, `sessions.stop`, `sessions.archive`, `sessions.delete`)**: It can tidy sessions into sidebar folders or stop/archive finished ones.
- **Read and write your Notes (`notes.list`, `notes.read`, `notes.write`)**: It can look up your saved notes in Vorkspace or jot down a new note for you.
- **Keep a to-do list (`todos.list`, `todos.add`, `todos.update`, `todos.remove`)**: It can add items to its board (or its project's board), move them between To do, In progress and Done, and tidy them up.

## How Bots Use Their Teams

With **Settings › Bots › Minions** enabled, a parent Bot can hire lasting team members with `minions.hire`, giving each a name, role and optional specialties. Hiring does not start a job or require picking a project. The parent checks `minions.list`, then reuses a member with `bots.send` for each new job. Its replies normally return automatically. The older `minions.spawn` shortcut hires a member and sends a first job together. These team actions belong to Bots; typing `/operator` in an ordinary session does not grant them.

Members keep their own conversations but share their parent's instructions, saved memory, to-do board and current project assignments. They can talk only with their parent and siblings, directly or in a channel containing just that team. Unrelated Bots cannot message them or read their conversations. Ordinary Bots can still collaborate with each other as usual.

You can inspect a member's work and past conversations, but its conversation and team channels are read-only for you. Ask the parent Bot to send instructions. Minions cannot hire or fire other members, open more work sessions, or change their parent's personality or habits.

When someone is no longer useful, the parent uses `minions.fire` to retire them. Their conversations stay saved, but they stop accepting work. Old queued messages do not restart after reopening the app. See [Minions](minions.md).

---

## How Vorkspace Keeps It Safe

- **Opt-In per Thread**: Only the thread where you typed `/operator` gets the walkie-talkie. Other chats cannot use it unless you give it to them too.
- **Only While Working**: The agent can only run Vorkspace commands while it is actively answering your turn.
- **Protected Chats Stay Protected**: The agent cannot stop or remove itself, Bot chats, scheduled habit runs, channel workers, orchestration workers, or private internal chats. A Bot can still follow up with or stop ordinary workers it started, even when they are hidden from the sidebar. Hidden chats with unknown ownership stay protected. Removing a chat keeps its files, terminals, and Git worktrees.

- **Your Draft Stays Yours**: A follow-up cannot replace text or files you are already preparing. Loading chats wait until their saved queue and draft are known.
- **Launch Choices Matter**: New sessions use the requested permission mode, model settings, and project checkout. An unrelated folder or misspelled option is rejected before a session is created. It can also split beside an open chat to the right or below. If that chat closes while the new session is being prepared, the launch is rejected instead of appearing beside an unrelated chat.
- **Failed Starts Clean Up**: A new chat appears only after its message is saved. If starting it fails, the app removes that unused chat and its unchanged new worktree. It keeps and reports anything that someone else has changed or taken over.
- **Retries Remember What Happened**: Changes get a saved request ID before they start. New prompts are saved before the agent receives an answer, so closing the app before a worker starts does not lose them. Ordinary reopened session queues wait for review. Saved Bot-to-Bot messages can resume after permission and delivery checks; retired members stay inactive. Sending the same request again returns its saved answer instead of repeating the change. If the app closes before recording an answer, it says the result is uncertain and does not run it again. Check what happened before requesting a new change. Reading a list simply reads the current state.
- **Bot Reports Stay Scoped**: Managing a chat stops that Bot from watching it for a future completion report. Other Bots keep their own monitoring; an already queued message remains saved.

If a direct task fails, later work waits. The parent can inspect what happened, then retry or discard that task. A retry starts a new attempt; it does not assume the earlier attempt did nothing.

Bots give delegated work that edits files its own worktree, then clean up. When work is finished they archive its sessions and remove the worktree, but only if nothing would be lost. A worktree with uncommitted or unmerged work, an open terminal, or a live session is left alone, and the Bot tells you instead. There's no way for a Bot to force it.
