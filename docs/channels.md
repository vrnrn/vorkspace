# Bot Channels

Channels use existing conversation owners for provider execution, approvals, Stop, and persistence. Opening a channel does not start work. Voice controls are documented in [Vox](vox.md).

Open a Bot and choose **Channels**. Create a named channel with member Bot names
and an optional topic, then send a message. A human message without mentions
wakes its members; explicit mentions target selected members. Bot replies wake
only mentioned members, with a maximum of four autonomous hops.

Each member has a separate backing conversation that inherits its provider,
model, account and Bot configuration. Approval controls remain available in the
channel. **Inspect conversation** shows its full native conversation. **Stop**
interrupts a delivery; interrupted deliveries can be dismissed after inspection.
Drafts persist independently from channel history.

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

See [verification](testing.md) and [runtime boundaries](architecture.md).
