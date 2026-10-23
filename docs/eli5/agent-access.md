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

---

## How Vorkspace Keeps It Safe

- **Opt-In per Thread**: Only the thread where you typed `/operator` gets the walkie-talkie. Other chats cannot use it unless you give it to them too.
- **Only While Working**: The agent can only run Vorkspace commands while it is actively answering your turn.
- **Protected Chats Stay Protected**: The agent cannot stop or remove itself, hidden chats, Bot chats, scheduled habit runs, channel workers, or orchestration workers. Removing a chat keeps its files, terminals, and Git worktrees.

- **Your Draft Stays Yours**: A follow-up cannot replace text or files you are already preparing. Loading chats wait until their saved queue and draft are known.
- **Launch Choices Matter**: New sessions use the requested permission mode, model settings, and project checkout. An unrelated folder or misspelled option is rejected before a session is created. It can also split beside an open chat to the right or below. If that chat closes while the new session is being prepared, the launch is rejected instead of appearing beside an unrelated chat.
- **Retries Remember What Happened**: Changes get a saved request ID before they start. New prompts are saved in the normal message queue before the agent receives an answer, so closing the app before a worker starts does not lose them. Reopened queued work waits for your review. Sending the same request again returns its saved answer instead of repeating the change. If the app closes before recording an answer, it says the result is uncertain and does not run it again. Check what happened before requesting a new change. Reading a list simply reads the current state.
- **Bot Reports Stay Scoped**: Managing a chat stops that Bot from watching it for a future completion report. Other Bots keep their own monitoring; an already queued message remains saved.
