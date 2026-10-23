# Data selection and continuity

Development builds use an isolated `vorkspace.db` under `vorkspace Native Development` in the platform's application-data directory. `VORKSPACE_NATIVE_DATA_DIR` overrides that directory. Release builds use the existing desktop store.

```sh
npm run dev -- --dev-data
npm run dev -- --vorkspace-data
npm run dev -- --vorkspace-db /absolute/path/to/store.db
```

`--check-data` opens the selected store read-only, prints its validation result, and exits. Fixture flags are allowed only with isolated development data.

The default production location is `com.vorkspace.desktop/vorkspace.db` under macOS Application Support, Linux's XDG data directory, or Windows `%APPDATA%`. If it is absent, the core recognizes the earlier application's directory and database by their exact persisted identity fingerprints. It opens that database in place; it does not copy files, rename the user's data, or invent replacement sessions.

The desktop schema remains readable without the former UI. Historical preferences are read from the matching WebKit store on macOS and normalized into current app keys. Native sidecar reads translate earlier Bot roster and viewport field names. Canonical keys take precedence when both versions exist. Existing Vox enabled, silent, and microphone preferences participate in the same read-time reconciliation, preserving later native choices. Transcript text, arbitrary document JSON, paths, and provider IDs remain untouched. Saving uses the current metadata names and atomic sidecar replacement.

Bot memory and habit files use their existing owner directory in the selected data directory. New owners use `bots/`; earlier named or project-indexed owners stay where they are, including their provider homes. Older transcript tables remain the write authority for their sessions until an existing completed conversion or a populated current owner establishes the current identity. Empty placeholder directories and transcript roots do not hide saved owners. No files or rows are moved when reading these aliases. Remote host databases remain on their host.

New Bot setup stays in memory until the user supplies a name and confirms Create. Cancelling setup creates no roster entry, backing session or Bot files. Confirmation saves the named identity with its selected provider configuration before creating its backing session. Those setup choices remain in the roster until the session's saved settings and published session ID take ownership, so reopening during setup completion retains the selected configuration. Existing Bot names and conversations remain unchanged.

## Unsent composer state

Normal session drafts use the existing `composer_drafts` table. Text, UTF-16 selection, attachment order and metadata, and MCP tags are restored when the session opens. Native edits save through a coalesced writer, with a final flush on disposal or quit. Reads and unchanged restores do not rewrite the row. Revision checks reject conflicting writers; clearing leaves a revision tombstone so an older writer cannot recreate a deleted draft. A failed compare-and-save preserves the pending variant in per-writer SQLite recovery rows. Reopening shows a preview and explicit restore or keep-saved choices. Restoring compares the current revision and removes only the selected recovery row after the chosen draft is durable; other windows’ variants remain available. Missing attachment references or malformed saved state remain recoverable in storage and block sending until resolved. Bot workspace drafts retain their separate workspace owner.

## Incremental history writes

Saved history uses ordered block rows so a streaming update can replace its changed tail without loading or rewriting unchanged tool output. Existing JSON histories are converted inside their first successful write transaction; read-only inspection does not convert them. A loaded view retains a compact history index and revision guard. External history edits invalidate that guard, and a stale writer must reopen the conversation before saving.

Changed blocks are merged with their complete durable source before publication, preserving tool fields omitted from the display. History changes, message receipts, queued-message removal, and their revision commit together. A failed transaction leaves both the durable history and the view's committed index unchanged.

The remote host similarly stores ordered blocks separately from session metadata. Its saved block values are immutable; a changed block receives a new value and sync revision. Metadata, block changes, event records, and command receipts share one SQLite transaction. A full history is reconstructed when requested, while unchanged polling reads only revision metadata.

The host reads both complete JSON snapshots and normalized block storage, converting a legacy session on its next successful save. Host programs that require all blocks inside `sessions.snapshot` cannot read a converted session; do not downgrade a converted database to those programs. Desktop disconnection and reconnection do not change the host's storage owner.

## Local submission receipts

Local sends reserve their stable message identity before provider admission. The durable receipt advances before provider input, then records completion or an uncertain outcome. Startup reconciles receipts whose process owner has exited; it never automatically replays a send whose external effects are unknown. An unconfirmed send remains visible for review. Restoring its text to an empty composer requires an explicit action, and dismissing the notice retains its no-replay identity. Session deletion retains receipt tombstones so delayed send work cannot recreate deleted state.

## Private composer attachments

Pasted images are cached under `composer-attachments/` in the selected data directory. Publication follows exclusive file creation, file flush and directory flush; Unix directories and files use owner-only permissions. Drafts, queued messages, history and receipts retain references to those files.

A cold-start worker may collect old, unreferenced cache files. It scans durable references outside the interactive path, uses a publication grace period and bounded scan, and defers collection when another live process owns the cache or writes invalidate its snapshot. Unknown files, symlinks, incomplete scans and malformed storage preserve files. Opening with `--check-data` neither registers this owner nor starts collection.
