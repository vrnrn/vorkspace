# Bot Channels

Channels use existing conversation owners for provider execution, approvals, Stop, and persistence. Opening a channel does not start work. Voice controls are documented in [Vox](vox.md).

Choose a channel under **Channels** in the main sidebar, or use its **+** button
to create one. Choose member Bots and an optional topic; the gear at the right
of the header opens a channel settings popover for the name, topic and members.
Type `@` in the composer to choose a member Bot; Enter or Tab inserts the
mention, and Escape dismisses suggestions. The sidebar shows channels as `#name`; a dot
marks unread messages and clears when you view the conversation. Read markers
are saved across restarts. Conversations render Markdown and keep a separate
composer: Enter sends and Shift+Enter adds a line. A human message without mentions
wakes its members; explicit mentions target selected members. Bot replies wake
only mentioned members, with a maximum of four autonomous hops.

Each member has a separate backing conversation that inherits its provider,
model, account and Bot configuration. Approval controls remain available in the
channel. **Inspect conversation** shows its full native conversation. **Stop**
interrupts a delivery; interrupted deliveries can be dismissed after inspection.
Drafts persist independently from channel history.
Sending publishes the message, delivery intent and cleared draft atomically.
Members with pending turns cannot be removed until those turns are stopped and
dismissed. Completed backing conversations remain available for inspection.

`crates/vorkspace-core/src/bot/channels.rs` owns message and delivery intent. It imports
desktop channel messages and backing identities without replacing unresolved
history. Native changes use `bot-channels-native-v1` in the native sidecar;
desktop channel storage is not rewritten. The native store limits channels,
retained message bytes and pending deliveries. Corrupt imports block mutation.

A delivery is durably marked running before dispatch. After a crash, running
deliveries become interrupted and are never automatically replayed: an external
effect may already have happened. A reply and its next deliveries publish in one
saved state. Request identities prevent repeated posts with the same payload.
Bots can use `channels.list`, `channels.create`, `channels.read` and
`channels.post` through their existing scoped app capability. A channel worker's
final reply is posted automatically, so recursive channel posts are rejected.

## Direct Bot messages

The same scoped app capability exposes `bots.list`, `bots.read` and `bots.send`.
Read returns a bounded slice of a peer's primary conversation, excluding tool
and reasoning rows. It runs off the UI thread and projects at most 100 source
records per page. Indexed owners seek directly; legacy JSON scanning is bounded
by the document limit. Use its opaque `before` cursor with `hasMore` even when a
tool-only page contains no public messages. Selected text is truncated before
leaving SQLite; each message reports `truncated`. Remote history, unindexed
documents over 8 MiB and selected shared blocks over 1 MiB return an explicit
read-limit error rather than loading the full source. Reads do not migrate or
rewrite history. Send accepts a Bot name or ID, message and optional
`expectReply` (default true). The recipient keeps its own model, account,
permissions and assigned projects. Direct messages use its durable primary
outbox and wait while it is busy. The final assistant message returns to the
sender as a one-way message; intermediate commentary is not forwarded.
The primary conversation displays the incoming body in a **Bot-to-bot** block
with sender and recipient names. Its answer identifies the peer it addresses;
automatic replies and one-way follow-ups use different labels. These labels
are derived from the saved internal envelope, including existing histories,
and remain visible on queued or failed deliveries.

Direct and channel messages share the four-hop autonomous budget. Switching
routes or creating another channel does not reset it. Each peer queue admits
at most 64 messages and 1 MiB of prompt text; individual messages and final
replies are limited to 65536 bytes. Full replies remain in their source
conversation when routing fails. Failed or cancelled peer deliveries stay
visible for explicit retry or discard. After restart, already-submitted peer
messages pause for inspection rather than replaying unknown external effects.
Queued, unsubmitted messages can continue normally. Command receipts fence
duplicate API submissions.
Automatic usage recovery preserves uncertain peer deliveries for inspection;
explicit Retry publishes a fresh submission identity while retaining the old
receipt's replay protection.

See [verification](testing.md) and [runtime boundaries](architecture.md).
