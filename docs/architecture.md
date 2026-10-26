# Architecture

## Desktop ownership

`crates/vorkspace-gpui` owns windows, focus, layout, input, rendering, and interactive views. `crates/vorkspace-core` owns provider transports, persistence, filesystem/Git operations, account configuration, Bot state, and application effects. Both execute in one desktop process; there is no separate local daemon.

A conversation admits a turn, records its submission state, starts the selected provider CLI through the core process owner, and projects streamed events into the visible transcript. Approvals and Stop remain routed through that owner. Durable receipts and revision checks distinguish accepted work from unknown or stale results. Closing or hiding a view does not mean its conversation was deleted.

The core reads and writes the desktop SQLite schema directly. Native viewport and supplementary state live in an atomically written `.gpui.json` sidecar. Bot files live under the selected data directory. History projection preserves unloaded fields and canonical provider session IDs; conflicting writes fail instead of replacing newer state. [Data continuity](data-storage.md) explains historical-data reads.

## Rendering and history work

Indexed histories are projected one source block at a time, so loading display rows does not retain a second complete parsed transcript. Durable tool payloads remain in storage; display metadata excludes large output fields. Preference-only writes do not reconstruct historical turn receipts.

Streaming persistence writes changed blocks against a guarded durable revision. Ordered history rows separate large immutable payloads from frequently updated session metadata. The desktop and remote host each own their database transactions, including submission receipts; a GUI projection cannot replace unloaded fields. See [incremental history writes](data-storage.md#incremental-history-writes).

Bot card refreshes use an application-owned, scoped persistence worker. Admission coalesces each card without taking the database lock; the worker compares the captured card with its durable block by ID and changes only that metadata. It preserves external references, opaque fields, message boundaries and later history. Warm revision guards advance only across this owner’s commit. Cold digest guards retain only the owning blocks changed locally until the next history load or save, so a provider tail save can validate those exact changes without accepting external edits or stale card rows. Successful receipts publish only into the matching session, submission generation, card identity and current value. Failed or conflicting saves keep the old visible value and release admission for a later refresh. Navigation or view disposal does not cancel admitted writes. Application quit closes admission and joins the worker synchronously because GPUI bounds asynchronous quit cleanup to 200 ms; shutdown can therefore wait for storage. Forced termination before commit can lose an unpublished refresh intent.

A transcript view owns one elapsed-time task while it is visible and streaming. Terminal grids stay current when hidden; repaint-only signals are coalesced before they reach the UI queue, while ordered PTY, clipboard, and exit effects are preserved. Editor analysis shares the immutable Git baseline until a Git refresh replaces it.

Bot chats keep their configured face and name in a fixed overlay within the transcript viewport. The existing conversation status drives idle, working and attention states, with attention taking precedence. The header shares the avatar image cache but has no pointer-tracking input. On macOS, paired native group markers fade transcript content around the face back to the already-rendered background, preserving its colors, texture and transparency. The radial feather composes with the transcript's edge fade and stays outside composer and attention controls. Decoration does not intercept chat input, and hidden or clipped content and inactive windows suppress automatic animation frames. The empty-chat greeting contains help text rather than a second face.

Every click of the Terminal sidebar button opens and focuses a new regular title tab for the selected local project or worktree directory. `shell/terminal_tabs.rs` retains one `Terminal` entity per stable terminal-tab ID, independently of its starting directory and the project's dock entities. Multiple tabs in the same directory have separate shells, output and close ownership; clicking an existing title tab selects that exact shell. Selecting another tab hides the grid without terminating its PTY or changing its cwd. Terminal tabs share title-tab selection, drag order, close menus, and workspace persistence; only identity and starting cwd are saved, with a fresh shell created on first selection after restart. Legacy cwd-only saved tabs acquire IDs before order deduplication, with scope focus mapped to the corresponding tab. Explicit close checks the foreground process, asks before closing a running job, and releases the entity through `Terminal::drop` (PTY shutdown and Unix shell/process-group hangup). Hidden tab output updates its grid without observing or notifying Shell. Existing dock menu commands and shortcuts remain available and own separate shells.

Parked provider leases are removed under their pool lock, then destroyed after releasing it. Cancellation epochs prevent a settling cancelled lease from reappearing after its binding is invalidated.

Provider transports admit encoded output before growing their buffers. A single encoded provider event frame is limited to 128 MiB; each transport shares a 256 MiB allocation budget across frames being read, queued, and processed, with at most 16 queued frames. A full budget applies backpressure; an oversized frame fails explicitly instead of being truncated. Closing the receiver wakes blocked producers before their threads are joined. These limits cover encoded transport buffers, not the provider subprocess or parsed conversation state.

Interface scale is limited to 60%–160% and uses a per-window GPUI transform, separate from display DPI. The window maps platform input and text-input geometry to the same logical layout used for rendering and accessibility. Native HTML and video hosts convert their clipped bounds at the platform boundary; changing scale invalidates cached paint geometry.

## JavaScript boundaries

- Editor analysis and formatting run committed bundles in bounded QuickJS interpreters. Rust owns files, edits, and saves.
- Vox runs a Rust controller that owns its Codex app-server child, bounded tool receipts, saved conversation identity, memory, and shutdown. An ephemeral WebKit WebRTC page handles microphone and audio only. Window-scoped GPUI handlers admit actions through existing session and Bot owners.
- Claude sessions routed through Copilot use a Rust loopback gateway. The supervised Claude process owns its gateway across parked turns; terminating that process releases the listener and connected clients. The remote Node host retains its JavaScript gateway. Other provider transports remain Rust-owned local CLI processes.
- The remote Node host owns its own database and provider processes. SSH carries authenticated RPC through a private forward; local windows consume revisioned remote snapshots.

JavaScript is maintained directly under `host/` and `embedded/`. No browser frontend or TypeScript compilation step is part of the application build.

## Platform services

GPUI renders the desktop. WebKit is limited to Vox and sandboxed HTML previews. Sparkle can verify and install explicitly configured macOS updates.

Bots are opened inside the main workspace. The macOS menu bar opens clipboard history, also available with ⌘⇧C. General settings control the icon and contain a Clipboard group between Workspace and About. Quick Composer, Settings and Quit remain available in the clipboard menu.

Clipboard history has one process-local owner with at most 50 entries and a 64 MiB retained payload budget, including normalized search text and reserved thumbnail and native presentation bytes. Individual images are limited to 16 MiB encoded and text to 1 MiB; larger copies are skipped without displacing existing entries. Nothing is persisted. Eviction and Clear release originals and bounded native thumbnails; clearing history leaves the system clipboard alone. Turning recording off clears history and stops its polling task and shortcut registration. A thin Swift/AppKit panel owns presentation only, using system menu material and controls. Its preview pane appears only after a continuous one-second image hover; leaving the image, filtering, clearing or closing cancels the preview. Closing releases native presentation copies. With recording off, the popup shrinks to Settings, Quick Composer and Quit; clipboard controls and previews are absent.

A 500 ms timer reads only the native pasteboard change counter until it changes. Confidential, transient and autogenerated pasteboard types are ignored, following Maccy's MIT-licensed rules. ImageIO creates a maximum 256-pixel preview on a single background worker, retaining the encoded original for copying. There is no decode queue or full-size preview cache. Native decoder working memory and the one in-flight original are temporary and separate from the retained payload budget; epoch and entry identity checks prevent cleared or evicted images from reappearing. Text search uses precomputed lowercase strings and the list renders visible rows only.

The [repository map](repository.md) lists the source owners and generated assets.

[Plain-English companion](eli5/architecture.md).
