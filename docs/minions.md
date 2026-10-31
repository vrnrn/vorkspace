# Minions

Minions are persistent members of a Bot's team. Each has a name, role and optional specialties. A parent Bot hires a useful role, delegates repeatedly to the same member and fires members it no longer needs. Enable **Settings › Bots › Minions** to use the experimental feature; it is off by default and requires Bots.

## Hire, reuse and retire

These actions use the authenticated `app` CLI supplied to a Bot's provider turn. Use the executable path shown in that turn's context. Inputs match `app help`:

| Action | Input | Effect |
|---|---|---|
| `minions.hire` | `{name,role,specialties?,project?}` | Creates a persistent member without queueing a task. |
| `minions.list` | `{}` | Lists the caller's team members, including retired members. |
| `minions.retry` / `minions.discard` | `{minion,messageId}` | Parent-only recovery of a failed direct task after inspecting its history. Retry uses a fresh submission identity. |
| `minions.fire` | `{minion}` | Retires a member belonging to the calling parent; accepts its ID or unambiguous name. |
| `minions.spawn` | `{prompt,name?,role?,specialties?,project?,harness?,model?}` | Compatibility action: hires a member and queues an initial peer task. |

Hiring requires neither an assigned project nor a project selection. If supplied, `project` must identify one of the parent's assigned projects; it validates that choice without pinning the member's scope. Members follow the parent's live project assignments. Hiring does not create a project session, isolate a worktree or start a provider. A backing Bot conversation is loaded or created lazily when needed.

Names contain 1–60 characters after whitespace normalization. Roles contain 1–1024 bytes. `specialties` is an array of up to 32 nonempty strings of at most 160 bytes each. An active name already used in the same team is rejected so the parent can reuse that member's ID. The compatibility action supplies the next free `Minion N` when no name is given and `Team helper` when no role is given. Only that action accepts `harness` and `model` overrides; new members otherwise inherit the hiring caller's execution configuration when available.

`minions.list` returns `id`, `name`, `parentId`, `role`, `specialties`, `retired`, `createdAt`, nullable `sessionId`, `working`, `status` and the parent's current `projects`. Status is `idle`, `working`, `needs-you` or `retired`, accounting for both direct and channel work. `pendingMessages` lists queued message IDs and errors. A failed message pauses later work until the parent inspects history and explicitly retries or discards it. Never retry uncertain effects blindly. Hire and compatibility-spawn responses return the same member description with `accepted:true`; acceptance does not mean the task has finished.

For work, use `bots.send {to,message,expectReply?}` with the member's stable ID or unambiguous team name. `expectReply` defaults to true. The app routes the final reply back automatically, subject to the shared autonomous-hop budget, so the sender should end its turn instead of polling or asking for a duplicate reply. `bots.read {bot,before?,limit?,maxChars?}` reads saved conversation history. Each member retains its own conversation across tasks. `sessions.send` and one-shot delegated-session completion groups are not the team messaging protocol.

## Actor identity and shared storage

A member is a `Bot` in the persisted roster with optional `Bot.minion` metadata: `parentId`, `role`, `specialties`, `retired` and `createdAt`. Its own Bot ID owns its outbox, conversation and message sender identity. The parent's Bot ID owns the shared soul, memory and default to-do board. Peer messages and channel posts retain the member as author; sharing storage never turns its messages into messages from the parent. Restored views use the member's Bot capability for execution; a former project pane's Operator environment cannot replace it.

`Roster::effective_bot` constructs an execution snapshot with current parent projects and team context. `Bot::storage_id` and `Roster::storage_owner` select the shared owner. Turns use the existing Bot executor, durable outbox, submission receipts and bounded peer/channel queues. Runtime loads shared soul and memory through the parent storage ID and sends role, specialties, active teammates and project scope as context. Context is resent when it changes or a fresh provider thread needs it.

`memory.*` addresses the parent's memory. `todos.*` addresses the parent's board unless `project` selects one of its assigned project boards. Notes and project-session/worktree reads use the parent's assigned project scope. Minions may also use `models.list`, `soul.read`, `artifacts.list/read`, `minions.list`, and the team communication actions. They cannot hire or fire members, start or control sessions, create worktrees, change soul or manage habits. Provider execution still uses the selected access policy.

## Team-only communication

Direct sends and peer-history reads involving a Minion are limited to its parent and siblings in the same team. Sending requires active participants; saved history of a retired member remains readable within its team. `bots.list` filters out unrelated members. Top-level Bots retain their ordinary peer relationships with other top-level Bots.

Channels use these inputs:

- `channels.list {}`
- `channels.create {name,topic?,members:[Bot names or IDs]}`
- `channels.read {channel,limit?}`
- `channels.post {channel,message,to?:[Bot names or IDs]}`

A channel containing Minions must contain members of one team only: that parent and its members. Reusing a channel name cannot merge a team with unrelated Bots. Team ownership remains recorded after member edits or retirement. Channel reads and posts require active Bot membership, and delivery admission checks the recipient and trigger author again. Explicit `to` targets or `@Name`/`@ID` mentions request the next member's turn. A channel worker's final reply is posted automatically; it cannot also post through the CLI during that turn. Direct and channel routes share the same four-hop autonomous budget.

`channels.list` exposes `teamParent` and `userReadOnly`; `Channel::user_read_only` provides the same classification to native views. Users may inspect team conversations but cannot send to a Minion or post in its team channels. Instructions go to the parent Bot. Supervision surfaces must keep user composers, voice submission and retry/resume paths from injecting member messages.

## Retirement, history and presentation

Firing retains the identity, backing conversation and channel history. It revokes active app admission and waits on the background executor for already executing app effects before acknowledging retirement, clears the member's outbox, stops loaded execution views, pauses saved follow-up queues and cancels channel deliveries that membership no longer permits. Queued peer messages from a retired sender become errors. Reopening revalidates team queues; retirement does not authorize replay of unfinished work. Legacy user follow-ups in migrated conversations remain paused.

Legacy `Bot.minions` records migrate to persistent Bot identities with stable IDs, preserving their existing session associations and transcript history. Their old one-shot launch monitoring is dismissed. Backing conversations are protected Bot chats and stay out of project-session lists.

Minions belong beneath their parent in Bot navigation. Their one-eye avatar uses the parent's live colour; independently selected eye style, body shape and pattern remain stable for the member's identity. Users supervise saved activity and history through read-only member views, and ask the parent to delegate further work.

See [agent access](agent-access.md), [Bot Channels](channels.md), or the [plain-English companion](eli5/minions.md).
