# Agent Access with `/operator` (ELI5)

Normally, an AI agent in a chat tab only works inside its own conversation and its own project folder. It doesn't know how to click buttons inside Vorkspace itself.

When you start a message with **`/operator`** (or chat with a **Bot**), you hand that agent a special walkie-talkie (`vorkspace app`) that lets it control Vorkspace for you during that conversation.

For the full reference, see [Agent access with /operator](../agent-access.md).

---

## What Can an `/operator` Agent Do?

Once you type `/operator` in a thread, that agent can:

- **Start new agent tabs (`sessions.start`)**: For example, you can say *"/operator open two tabs—one to check the backend and one to check the UI"* and it will create new tabs for you.
- **Explore project code**: With [Codebase Memory](codebase.md) enabled for a project, agents can find symbols, read source previews and follow calls. A chat uses its own working copy’s map. Read-only chats and Minions can search, but cannot enable it or start indexing.
- **Create Git worktrees (`worktrees.create`)**: It can set up fresh branch folders so multiple agents can work in parallel without stepping on each other's code.
- **Check on other tabs (`sessions.list`, `sessions.read`)**: It can look at your other sessions in the project and read their recent replies.
- **Arrange workspace tabs (`workspace.list`, `workspace.open`, `workspace.focus`, `workspace.close`, `workspace.move`, `workspace.detach`, `workspace.reorder`, `workspace.resize`)**: It can see which panes are open, open a note, connector item, Bot, document, terminal, Search, file, or chat, then select, tile, reorder, resize, or close those panes. It can move one pane or a whole tiled tab, using the same room limits as dragging panes yourself. If a terminal is running a process, the agent must inspect the process and explicitly confirm that exact process before closing it.
- **Send follow-ups or drafts (`sessions.send`, `sessions.draft`)**: It can message an idle session or leave a draft message in its box for you to review before sending.
- **Organize and clean up (`folders.move`, `sessions.stop`, `sessions.archive`, `sessions.delete`)**: It can tidy sessions into sidebar folders or stop/archive finished ones.
- **Read and write your Notes (`notes.list`, `notes.read`, `notes.write`)**: It can look up your saved notes in Vorkspace or jot down a new note for you. The CLI shows each note's exact native `noteId` as well as its older numeric ID. If that note has an unfinished editor or recording change, the agent's write waits so your draft stays intact.
- **Keep project to-dos (`todos.list`, `todos.add`, `todos.update`, `todos.remove`)**: It can add tasks with descriptions, attachments and dependencies, move them through To do, In progress, In review and Done, and work without GitHub. A linked project's GitHub issues also appear; publishing a local task to GitHub is an explicit choice. See [To-dos](todos.md).

Bots and Minions already have the commands they can use in their instructions. They can read their saved soul and memory through Vorkspace when you ask, including topic notes and archived facts.

## How Bots Remember Long Conversations

In each Bot's General settings, choose how it handles a long conversation. You can also ask the Bot to change its own setting with `bots.configure`. The change applies to later messages. **Fresh session**, the default, starts a new provider session before the next message once context is 80% full. It brings a short recap plus the Bot's instructions and saved memory. Your full app chat stays in place, and the context ring resets for the new session. Older provider sessions remain in history, using the same Bot folder.

Choose **Native compaction** to keep one provider session and let the provider shorten its working context. You can also use `/compact` with Codex, Claude, Pi or OMP. Before compacting a Bot, Vorkspace catches up with its terminal conversation and waits if an external turn is unfinished. Vorkspace prevents another app turn or history import from overlapping compaction, and its context reading cannot overwrite a replacement session's reading. Neither mode discards your queued messages or draft.

On the next turn after a recorded compaction attempt, Vorkspace gives the Bot its current instructions and saved memory again. This also works after reopening the app. Normal turns do not repeat those instructions unless they changed.

Saved memory is for lasting things: your preferences, decisions, constraints and useful lessons. Bots are told to save these as they learn them, keep them short, and include references when useful. You can edit memory in Bot settings. Longer notes remain searchable. A remembered task status may be old, so Bots must check the app's current sessions and to-dos before relying on it. Compaction itself does not run another model to write memories.

## A Folder for Each Bot

When you open a Bot, Vorkspace gives it a working folder at `~/.vorkspace/bots/<name>`. Inside are its personality and instructions (`SOUL.md`), short saved memories (`MEMORY.md`), and longer notes (`memory/`). The folder points to the Bot's existing files, so editing them there changes the same files you see in Bot settings. Nothing is copied or cleared. Renaming the Bot keeps its existing folder, and duplicate names get a unique suffix.

`SESSION.md` tells you how to reopen the same Codex or Claude conversation from a terminal, using the right account. The provider still keeps its conversation history in its normal location. The Bot can continue working on its assigned projects under the permissions you selected. `AGENTS.md` tells an outside agent where to read the Bot's instructions and memory; you can edit this file.

Messages you add to the current Codex or Claude session through an outside CLI appear in the same Bot chat. Vorkspace checks when you open the Bot, about every 10 seconds while it is idle, and before sending another message. Repeated checks do not duplicate messages or replace your draft. In Fresh session mode, `SESSION.md` always points to the newest provider session.

Finish the Bot's current turn before switching between the app and a terminal, and finish the terminal turn before returning. Vorkspace waits if it sees an unfinished external turn. If the CLI crashed or was closed, choose **Resume here** in the Bot chat after making sure the CLI has stopped. This keeps the same conversation and never resends the old message. It shows any text already saved by that CLI before your next app exchange, preserving the conversation's order. New writes from the CLI cancel that confirmation. A Bot can also check another accessible Bot with `bots.historyStatus` and recover it with `bots.recoverHistory`, using the returned token and your explicit confirmation that its CLI has stopped. Vorkspace cannot prevent another CLI from writing at the same time. App commands are available only during a turn running in Vorkspace. Existing project-based Minions keep their working folders. Development copies of the app use a separate folder beside their own database.

## How Bots Use Their Teams

With **Settings › Bots › Minions** enabled, a parent Bot can hire lasting team members with `minions.hire`, giving each a name, role and optional specialties. Hiring does not start a job or require picking a project. The parent checks `minions.list`, then reuses a member with `bots.send` for each new job. Its replies normally return automatically. The older `minions.spawn` shortcut hires a member and sends a first job together. These team actions belong to Bots; typing `/operator` in an ordinary session does not grant them.

Members keep their own conversations but share their parent's instructions, saved memory and current project assignments. Their to-dos belong to those projects. They can talk only with their parent and siblings. Unrelated Bots cannot message them or read their conversations. Ordinary Bots can still collaborate with each other as usual.

You can inspect a member's work and past conversations, but its conversation is read-only for you. Ask the parent Bot to send instructions. Minions cannot hire or fire other members, open more work sessions, or change their parent's personality or habits.

When someone is no longer useful, the parent uses `minions.fire` to retire them. Their conversations stay saved, but they stop accepting work. Old queued messages do not restart after reopening the app. See [Minions](minions.md).

---

## How Vorkspace Keeps It Safe

- **Opt-In per Thread**: Only the thread where you typed `/operator` gets the walkie-talkie. Other chats cannot use it unless you give it to them too.
- **Only While Working**: The agent can only run Vorkspace commands while it is actively answering your turn.
- **Protected Chats Stay Protected**: The agent cannot stop or remove itself, Bot chats, scheduled habit runs, orchestration workers, or private internal chats. A Bot can still follow up with or stop ordinary workers it started, even when they are hidden from the sidebar. Hidden chats with unknown ownership stay protected. Removing a chat keeps its files, terminals, and Git worktrees.

- **Your Draft Stays Yours**: A follow-up cannot replace text or files you are already preparing. Loading chats wait until their saved queue and draft are known.
- **Launch Choices Matter**: New sessions use the requested permission mode, model settings, and project checkout. An unrelated folder or misspelled option is rejected before a session is created. It can also split beside an open chat to the right or below. If that chat closes while the new session is being prepared, the launch is rejected instead of appearing beside an unrelated chat.
- **Failed Starts Clean Up**: A new chat appears only after its message is saved. If starting it fails, the app removes that unused chat and its unchanged new worktree. It keeps and reports anything that someone else has changed or taken over.
- **Retries Remember What Happened**: Changes get a saved request ID before they start. New prompts are saved before the agent receives an answer, so closing the app before a worker starts does not lose them. Ordinary reopened session queues wait for review. Saved Bot-to-Bot messages can resume after permission and delivery checks; retired members stay inactive. Sending the same request again returns its saved answer instead of repeating the change. If the app closes before recording an answer, it says the result is uncertain and does not run it again. Check what happened before requesting a new change. Reading a list simply reads the current state.
- **Bot Reports Stay Scoped**: Managing a chat stops that Bot from watching it for a future completion report. Other Bots keep their own monitoring; an already queued message remains saved.

If a direct task fails, later work waits. The parent can inspect what happened, then retry or discard that task. A retry starts a new attempt; it does not assume the earlier attempt did nothing.

Bots give delegated work that edits files its own worktree. When the work is finished, they archive its sessions and can remove the worktree after its safety checks pass. The Bot keeps it and tells you if Git cannot check it, or if it contains unique work or private ignored files such as `.env`. An open terminal or unarchived session also blocks cleanup. The worktree list can show that a check is unknown.

If setup fails or gets interrupted, the app keeps passing the warning to agents until a turn carrying it finishes. A warning can reach more than one turn if sessions overlap or a turn ends early.
