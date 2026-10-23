# Architecture

## Desktop ownership

`crates/vorkspace-gpui` owns windows, focus, layout, input, rendering, and interactive views. `crates/vorkspace-core` owns provider transports, persistence, filesystem/Git operations, account configuration, Bot state, and application effects. Both execute in one desktop process; there is no separate local daemon.

A conversation admits a turn, records its submission state, starts the selected provider CLI through the core process owner, and projects streamed events into the visible transcript. Approvals and Stop remain routed through that owner. Durable receipts and revision checks distinguish accepted work from unknown or stale results. Closing or hiding a view does not mean its conversation was deleted.

The core reads and writes the desktop SQLite schema directly. Native viewport and supplementary state live in an atomically written `.gpui.json` sidecar. Bot files live under the selected data directory. History projection preserves unloaded fields and canonical provider session IDs; conflicting writes fail instead of replacing newer state. [Data continuity](data-storage.md) explains historical-data reads.

## Rendering and history work

Indexed histories are projected one source block at a time, so loading display rows does not retain a second complete parsed transcript. Durable tool payloads remain in storage; display metadata excludes large output fields. Preference-only writes do not reconstruct historical turn receipts.

Streaming persistence writes changed blocks against a guarded durable revision. Ordered history rows separate large immutable payloads from frequently updated session metadata. The desktop and remote host each own their database transactions, including submission receipts; a GUI projection cannot replace unloaded fields. See [incremental history writes](data-storage.md#incremental-history-writes).

A transcript view owns one elapsed-time task while it is visible and streaming. Terminal grids stay current when hidden; repaint-only signals are coalesced before they reach the UI queue, while ordered PTY, clipboard, and exit effects are preserved. Editor analysis shares the immutable Git baseline until a Git refresh replaces it.

Parked provider leases are removed under their pool lock, then destroyed after releasing it. Cancellation epochs prevent a settling cancelled lease from reappearing after its binding is invalidated.

Provider transports admit encoded output before growing their buffers. A single NDJSON or OpenCode event frame is limited to 128 MiB; each transport shares a 256 MiB allocation budget across frames being read, queued, and processed, with at most 16 queued frames. A full budget applies backpressure; an oversized frame fails explicitly instead of being truncated. Closing the receiver wakes blocked producers before their threads are joined. These limits cover encoded transport buffers, not the provider subprocess or parsed conversation state.

## JavaScript boundaries

- Editor analysis and formatting run committed bundles in bounded QuickJS interpreters. Rust owns files, edits, and saves.
- Vox runs a Rust controller that owns its Codex app-server child, bounded tool receipts, saved conversation identity, memory, and shutdown. An ephemeral WebKit WebRTC page handles microphone and audio only. Window-scoped GPUI handlers admit actions through existing session and Bot owners.
- Claude sessions routed through Copilot use a Rust loopback gateway. The supervised Claude process owns its gateway across parked turns; terminating that process releases the listener and connected clients. The remote Node host retains its JavaScript gateway. Other provider transports remain Rust-owned local CLI processes.
- The remote Node host owns its own database and provider processes. SSH carries authenticated RPC through a private forward; local windows consume revisioned remote snapshots.

JavaScript is maintained directly under `host/` and `embedded/`. No browser frontend or TypeScript compilation step is part of the application build.

## Platform services

GPUI renders the desktop. WebKit is limited to Vox and sandboxed HTML previews. Sparkle can verify and install explicitly configured macOS updates.

The [repository map](repository.md) lists the source owners and generated assets.

[Plain-English companion](eli5/architecture.md).
