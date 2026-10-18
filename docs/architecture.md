# Vorkspace Architecture

Vorkspace is a cross-platform desktop environment (macOS, Linux, Windows) and headless remote daemon for orchestrating coding agents across local repositories, Git worktrees, and remote SSH machines. It wraps installed agent CLIs—**Claude Code**, **OpenAI Codex**, **Antigravity**, **Pi**, and **omp**—into a unified workspace without acting as a token reseller or middleman API proxy.

For a plain-English summary, see the [ELI5 version](eli5/architecture.md).

---

## 1. Runtime Topology

Vorkspace operates across three execution boundaries:

```
+-----------------------------------------------------------------------+
| Desktop Webviews (React 19 + TypeScript)                              |
|  - Main Workspace (index.html -> src/main.tsx -> src/app/App.tsx)     |
|  - Floating Bot Companion (bot-chat.html)                            |
|  - Quick Composer & Quick Git Popup (quick-composer.html)             |
+-----------------------------------+-----------------------------------+
                                    | Tauri v2 IPC / Custom Protocols
+-----------------------------------v-----------------------------------+
| Native Rust Core (src-tauri/src/)                                     |
|  - Process & PTY Supervisor (harness.rs, pty.rs, background_work.rs)  |
|  - SQLite & Account Stores (session_store.rs, codex_bot_store.rs,    |
|    provider_session_transfer.rs)                                      |
|  - Git, Worktrees & Checkpoints (fs.rs, worktrees.rs, checkpoint.rs)  |
|  - Local Loopback Control Server (control.rs, control_cli.rs)         |
|  - Remote SSH Tunnel & Bootstrapper (remote.rs, remote_ssh.rs)        |
+-----------------+-----------------------------------+-----------------+
                  | Local Subprocess Pipes            | SSH Port Forward (127.0.0.1)
+-----------------v-----------------+ +---------------v-----------------+
| Local Agent CLIs                  | | Remote Host Daemon (host/)      |
|  claude / codex / agy / pi / omp  | |  - HTTP/JSON-RPC (server.ts)    |
+-----------------------------------+ |  - Headless Engine (engine.ts)  |
                                      |  - SQLite Store (store.ts)      |
                                      |  - Process Guard (provider-     |
                                      |    guard.mjs)                   |
                                      +---------------------------------+
```

### Multi-Mode Native Binary (`src-tauri/src/main.rs`)

Before initializing the GUI, the native `vorkspace` binary inspects its arguments and environment to support four operating modes:
1. **`SSH_ASKPASS` Helper (`ssh_askpass::maybe_run`)**: When invoked by OpenSSH during remote connection setup, connects to an ephemeral loopback socket using a one-time token to display passphrase/host-key prompts in Settings.
2. **Orchestration Control CLI (`vorkspace control <action>`)**: JSON CLI injected into orchestration lead sessions (`control_cli.rs`) to spawn, inspect, and gate worker sessions.
3. **Operator App CLI (`vorkspace app <action>`)**: JSON CLI injected into `/operator`-enabled sessions and Bots to manage sessions, split panes, create worktrees, and read/write notes.
4. **Desktop GUI (`monocode_lib::run`)**: Boots the Tauri v2 application, native menus, system tray, custom URI schemes (`monoartifact://`, `vorkspace://`), and SQLite stores.

---

## 2. Frontend Architecture (`src/`)

The frontend is organized into strict layers:
- **`src/app/`**: Top-level application composition (`App.tsx`), window lifecycle, boot splash, and global shell (`Sidebar`, `ProjectRail`, `ProjectSpaces`, `TitleBar`, `NewLayoutNotice`). Supports both the classic full-bleed workspace layout and an opt-in **Inset Glass Layout** (`html.layout-inset`) with compositor-driven horizontal project space slides (`useProjectSpaceSlide`), a bottom project dot switcher (`ProjectSpacesBar`), reorderable right-panel project tools, and a dedicated right-sidebar Git History tab while preserving title-bar Vox controls.
- **`src/features/`**: Domain modules containing `model/`, `ui/`, `hooks/`, and `data/`:
  - `sessions`: Transcript rendering (`AgentTranscript`, `AgentMarkdown`), measured history virtualization (`useMeasuredHistory`), turn checkpoints, differential persistence, and inline usage-limit account switching (`UsageLimitAccountPicker`, `usageLimit.ts`).
  - `workspace`: Binary-tree split-pane layout (`layout.ts`), unified title-bar workspace/view tabs (`appViewTabs.ts`), kept-mounted view containers (`AppViewSurface.tsx` using `inert` to preserve drafts and selection across tab switches), and workspace snapshot serialization.
  - `projects`: Recent project management (`recents.ts`) and project scoping (`projectScope.ts`, scoping Inbox, Notes, Automations, and individual Bot tabs to the active project while keeping project-less items globally visible).
  - `settings`: Appearance controls (`appearance.ts`) and 11 dual-scheme dark/light **Workspace Themes** (`workspaceThemes.ts`—`default`, `harbor`, `moss`, `ember`, `orchid`, `tidepool`, `dune`, `rosewood`, `glacier`, `arcade`, `ink`) that drive `--wt-*` CSS variables, `--syntax-*` code tokens, and native macOS window glass tints (`src-tauri/src/macos.rs`).
  - `bots`: Persistent cross-project **Bots** backed by `SOUL.md` and `MEMORY.md`, scheduled **Habits** (`botHabits.ts`), multi-Bot **Channels** (`botChannels.ts`), and usage-limit account recovery (`botUsageLimit.ts`).
  - `orchestration`: Lead/worker multi-agent task decomposition across isolated worktrees with write-scope enforcement.
  - `automations`: Scheduled cron/calendar tasks and polling triggers (GitHub, GitLab, Jira).
  - `inbox`: Unified triage center for GitHub PRs/issues, GitLab MRs/issues, and Jira tickets, including one-click **CI Auto-Repair** (`ciRepair.ts`).
  - `source-control` & `files`: CodeMirror 6 text editor, interactive Git commit graph (`GitHistoryGraph.tsx`), staging, PR creation, fuzzy file search, and a **Worker-Backed Unified Diff Engine** (`ReviewDocumentBuilder` in `unifiedDiffDocument.ts`, off-thread Lezer syntax tokenization in `diffHighlight.ts` + `diffHighlight.worker.ts`, VS Code Dark+/Light+ `--syntax-*` tokens in `syntaxTokens.ts`, and single-observer viewport windowing in `useDiffViewport.ts`).
  - `notifications`: Local subscription-isolated resource monitoring (`ResourcePressureNotice.tsx`) that samples event-loop stall share and `resource_usage` outside the main workspace render loop, grouping owned child processes by session with granular agent vs. tool/build stop controls.
  - `vox`: Real-time voice controller using WebRTC and the OpenAI Codex app-server (`gpt-live-1-codex`).
  - `connections`: Remote machine management and `remote://<machineId>/<path>` routing.
- **`src/integrations/harness/`**: Provider-agnostic agent runtime (`core/registry.ts`, `core/child.ts`, `core/apply.ts`) and per-provider adapters (`claude`, `codex`, `antigravity`, `pi`, `omp`).
- **`src/platform/tauri/`**: Browser-to-native bridges for filesystem/Git (`fs.ts`), PTYs (`pty.ts`), clipboard (`clipboard.ts`), and OS capabilities.
- **`src/shared/`**: Pure UI primitives (`Modal`, `Popover`) and zero-dependency utilities (`paths.ts`, `numbers.ts`, `fuzzy.ts`, `concurrent.ts`).

### Fine-Grained Reactive State (`SessionStateStore`)

Streaming token deltas at 60+ Hz across multiple concurrent agents can easily overwhelm React's reconciliation tree if stored in a single top-level state object. `SessionStateStore` (`src/features/sessions/model/sessionStateStore.ts`) splits subscriptions into two channels via `useSyncExternalStore`:
- **Shell channel (`subscribeShell`)**: Fires only when sessions are created, archived, deleted, or change status/title/unread flags.
- **Per-session channel (`subscribe(sessionId)`)**: Fires on `textOnly` streaming updates, waking only the mounted `AgentTabView` displaying that specific session.

---

## 3. Native Storage & Persistence (`src-tauri/src/`)

All durable local state lives in the application data directory (`com.monocode.desktop` in production; `com.vrnrn.vorkspace.development` in `npm run tauri dev`):

- **SQLite (`monocode.db`)**: Managed by `session_store.rs` and `codex_bot_store.rs` in WAL mode (`PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;`) with a serialized write connection and a pool of read-only connections (`PRAGMA query_only = ON;`).
- **Incremental Block Storage (`session_store/incremental.rs`)**: Ordinary sessions persist individual message/tool blocks in `ordinary_blocks` rather than rewriting entire multi-megabyte JSON transcripts on every keystroke or turn. Frontend `WeakSet` tracking (`ordinarySaved`) ensures only dirty blocks are transmitted over IPC.
- **Deferred Tool Output (`tool_detail.rs`)**: Tool outputs exceeding 512 bytes are stripped from in-memory session snapshots and replaced with `detailOmitted: true` markers. The full payload remains in SQLite and is loaded on demand when the user expands the tool row.
- **Durable Submission Receipts (`local_submissions.rs`)**: Before dispatching a user prompt to a local CLI, a durable receipt is reserved on disk (`reserve` → `dispatch` → `settle`), preventing duplicate prompt execution or silent message loss across crashes.
- **Cross-Account Native Session Transfer (`provider_session_transfer.rs`, `codex_bot_store.rs`)**: When a Claude Code or Codex conversation hits an account usage limit, `transferHarnessSessionAccount` (`src/integrations/harness/core/registry.ts`) stops the active process, copies all native provider state to the newly selected account via `copy_durable` (flushed temp file + atomic rename), and rebinds the session:
  - **Claude Code**: Copies subagent transcripts (`projects/<cwd>/<id>/`) and edit checkpoints (`file-history/<id>/`) first, then publishes the root transcript (`projects/<cwd>/<id>.jsonl`) last so `--resume` never sees a partial copy.
  - **Codex**: Traverses spawned descendant threads (`thread_spawn_edges`), `.zst` compressed/plain JSONL sibling tails, and required inherited `history_base` rollouts across `sessions/` and `archived_sessions/`, commits closed subagent edges (`apply_agent_edges`) to the destination SQLite index first, and copies the root rollout last. When a custom shared `CODEX_SQLITE_HOME` is active or native state is unavailable (such as on remote hosts or private Bot stores), Vorkspace falls back to a structured transcript handoff (`accountContinuePrompt`).

---

## 4. Subprocess Supervision & Security Boundaries

- **Process Groups & Job Objects (`harness.rs`, `windows.rs`)**: Every spawned agent CLI is placed in its own Unix process group (`setsid` / `setpgid`) or Windows Job Object (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`). Startup reaping (`reap_orphaned_harness_processes`) cleans up stale descendants from prior crashes.
- **Resource QoS (`background_work.rs`)**: Child processes are configured with raised `nice` values and macOS `IOPOL_THROTTLE` I/O policies so background indexing or test suites do not starve the UI thread.
- **Sandboxed HTML Artifacts (`artifacts.rs`, `HtmlArtifactFrame.tsx`)**: Agent-generated HTML previews are served from the custom `monoartifact://` scheme inside `<iframe sandbox="allow-scripts">` **without** `allow-same-origin`, isolating untrusted scripts inside an opaque `null` origin with zero access to Tauri IPC.
- **Loopback Control Server (`control.rs`)**: Binds to `127.0.0.1:<ephemeral_port>` and issues 256-bit random hex tokens (`MONOCODE_CONTROL_TOKEN`, `MONOCODE_APP_TOKEN`) scoped to active agent turns.

---

## 5. Headless Remote Host (`host/`)

`vorkspace-host` is a standalone Node.js 24+ daemon bundled via `esbuild` (`build/host/vorkspace-host.mjs`) that runs on remote macOS, Linux, or Windows machines:
- **Transport**: Binds strictly to `127.0.0.1:3774` (`host/server.ts`). The desktop connects over an OpenSSH local port forward (`ssh -N -L`) and authenticates with SHA-256 hashed 256-bit device bearer tokens.
- **Headless Harness Reuse**: Reuses the exact TypeScript provider adapters (`src/integrations/harness/providers/*`) over `HostChildBackend` (`host/child-backend.ts`), supervising child trees via a dedicated file-descriptor pipe (`fd 3`) in `host/provider-guard.mjs`.
- **Claude Copilot Local Proxy (`host/claude-copilot.mjs`)**: When running Copilot GPT models inside Claude Code (`copilot/gpt-6.1-sol`, `copilot/gpt-6-luna`), spawns an ephemeral loopback HTTP translation proxy between the Anthropic Messages API and GitHub Copilot's `/responses` endpoint.
- **Chunked Sync & Bounded Cache (`host/store.ts`, `host/sync-transfer.ts`)**: Maintains a 16 MiB / 32-session LRU cache in front of `node:sqlite`, batches streaming writes at 120 ms intervals, and splits sync payloads larger than 4 MiB into surrogate-safe chunks.
