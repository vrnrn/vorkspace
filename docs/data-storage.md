# Data selection and continuity

All desktop launches use the existing production database by default, keeping sessions and project continuity persistent across builds. Isolated development data under `vorkspace Native Development` is used only when explicitly requested with `--dev-data`. `VORKSPACE_NATIVE_DATA_DIR` overrides the development directory. Without a directory override, the app resolves its data location from an absolute home directory, using the system account when the launch environment omits it.

```sh
npm run dev
npm run dev -- --dev-data
npm run dev -- --vorkspace-db /absolute/path/to/store.db
```

`--check-data` opens the selected store read-only, prints its validation result, and exits. Fixture flags are allowed only with isolated development data.

The default production location is `com.vorkspace.desktop/vorkspace.db` under macOS Application Support.

The session schema belongs to the app. Preferences, accounts, Bot identities, folders and Inbox choices come from the native settings sidecar. Transcript text, arbitrary document JSON, paths, and provider IDs remain untouched. Saving uses atomic sidecar replacement.

Bot memory and habit files use their owner directory under `bots/` in the selected data directory. A project-indexed owner there stays where it is, and an empty placeholder directory does not hide it. Codex Bot homes live under `codex-bots/`. No files are moved when a Bot opens.

New Bot setup stays in memory until the user supplies a name and confirms Create. Cancelling setup creates no roster entry, backing session or Bot files. Confirmation saves the named identity with its selected provider configuration and soul before creating its backing session. Those setup choices remain in the roster until its files and session settings are saved and its session ID is published, so reopening during setup completion retains the selected configuration. Existing Bot names and conversations remain unchanged.

## Unsent composer state

Normal session drafts use the existing `composer_drafts` table. Text, UTF-16 selection, attachment order and metadata, and MCP tags are restored when the session opens. Native edits save through a coalesced writer, with a final flush on disposal or quit. Reads and unchanged restores do not rewrite the row. Revision checks reject conflicting writers; clearing leaves a revision tombstone so an older writer cannot recreate a deleted draft. A failed compare-and-save preserves the pending variant in per-writer SQLite recovery rows. Reopening shows a preview and explicit restore or keep-saved choices. Restoring compares the current revision and dismisses the selected choice after the chosen draft is durable; other variants remain available. Ordinary recovery rows are removed at that point. A direct Bot submission keeps its durable receipt until outbox acknowledgement so reopening cannot submit it twice, while the resolved banner stays dismissed in the current view. Missing attachment references or malformed saved state remain recoverable in storage and block sending until resolved. Bot workspace drafts retain their separate workspace owner.

## Incremental history writes

Saved history uses ordered block rows so a streaming update can replace its changed tail without loading or rewriting unchanged tool output. Existing JSON histories are converted inside their first successful write transaction; read-only inspection does not convert them. A loaded view retains a compact history index and revision guard. Each active local turn retains its own save baseline independently of view reloads. If a competing edit changes its saved row boundaries or content, the app preserves the interrupted turn in a separate recovery record. The open conversation refreshes from its authoritative history while keeping the composer intact. **Recovered versions** shows the saved alternatives in that conversation and can copy the full text. If SQLite rejects the recovery write, the open app keeps a separate in-memory copy and lets the user retry or copy it.

The first explicit history write coordinates queue metadata, Bot draft origins, dispatch claims, local-send receipts and deletion tombstones, notes columns and indexes, the to-do table, and automation tables and indexes before publishing scalar revision guards. Their later writes use that prepared schema, so normal app activity cannot invalidate another open conversation through a schema change. The automation service migrates legacy event claims and publishes the migration marker in the same transaction. It also stores the trigger list for definitions saved before automations had one. That conversion first copies the database with `VACUUM INTO` to `backups/<UTC time>-definition-triggers-v1/` in the data directory. The folders are owner-only, and the copy is flushed and must open and pass `PRAGMA quick_check`. If the backup fails, the service does not open and nothing is converted. The rewrite and its `definition-triggers-v1` marker commit in one transaction. Fields the app does not know stay in the saved JSON. A partial index selects pending and running records for recovery, so settled launch prompts stay on disk. Unexpected schema changes suspend the scalar fast path until the next save verifies the complete rich history against the loaded block fingerprints; opening or inspecting a database remains read-only.

Changed blocks are merged with their complete durable source before publication, preserving tool fields omitted from the display. History changes, message receipts, queued-message removal, and their revision commit together. A failed transaction leaves both the durable history and the view's committed index unchanged.

## Local submission receipts

Local sends reserve their stable message identity before provider admission. The durable receipt advances before provider input, then records completion or an uncertain outcome. Startup reconciles receipts whose process owner has exited; it never automatically replays a send whose external effects are unknown. An unconfirmed send remains visible for review. Restoring its text to an empty composer requires an explicit action, and dismissing the notice retains its no-replay identity. Session deletion retains receipt tombstones so delayed send work cannot recreate deleted state.

## Local provider selection

The selected provider, model, thread binding, and native handoff preferences commit together in SQLite. A pending switch retains the outgoing thread ID, including an ID learned by the previous worker before selection commits. Selecting that provider again before the handoff starts restores its saved binding. A worker settles only the handoff it prepared, so a later selection retains its own state. Existing native agent settings remain readable from the sidecar until the first successful preference write for that conversation creates its SQLite record.

## Private composer attachments

Pasted images are cached under `composer-attachments/` in the selected data directory. Publication follows exclusive file creation, file flush and directory flush; Unix directories and files use owner-only permissions. Drafts, queued messages, history and receipts retain references to those files.

A cold-start worker may collect old, unreferenced cache files. It scans durable references outside the interactive path, uses a publication grace period and bounded scan, and defers collection when another live process owns the cache or writes invalidate its snapshot. Unknown files, symlinks, incomplete scans and malformed storage preserve files. Opening with `--check-data` neither registers this owner nor starts collection.
