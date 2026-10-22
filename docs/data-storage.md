# Data selection and continuity

Development builds use an isolated `vorkspace.db` under `vorkspace Native Development` in the platform's application-data directory. `VORKSPACE_NATIVE_DATA_DIR` overrides that directory. Release builds use the existing desktop store.

```sh
npm run dev -- --dev-data
npm run dev -- --vorkspace-data
npm run dev -- --vorkspace-db /absolute/path/to/store.db
```

`--check-data` opens the selected store read-only, prints its validation result, and exits. Fixture flags are allowed only with isolated development data.

The default production location is `com.vorkspace.desktop/vorkspace.db` under macOS Application Support, Linux's XDG data directory, or Windows `%APPDATA%`. If it is absent, the core recognizes the earlier application's directory and database by their exact persisted identity fingerprints. It opens that database in place; it does not copy files, rename the user's data, or invent replacement sessions.

The desktop schema remains readable without the former UI. Historical preferences are read from the matching WebKit store on macOS and normalized into current app keys. Native sidecar reads translate earlier Bot roster and viewport field names. Canonical keys take precedence when both versions exist. Transcript text, arbitrary document JSON, paths, and provider IDs remain untouched. Saving uses the current metadata names and atomic sidecar replacement.

Bot memory and habit files remain under `bots/` in the selected data directory. Remote host databases remain on their host.

## Incremental history writes

Saved history uses ordered block rows so a streaming update can replace its changed tail without loading or rewriting unchanged tool output. Existing JSON histories are converted inside their first successful write transaction; read-only inspection does not convert them. A loaded view retains a compact history index and revision guard. External history edits invalidate that guard, and a stale writer must reopen the conversation before saving.

Changed blocks are merged with their complete durable source before publication, preserving tool fields omitted from the display. History changes, message receipts, queued-message removal, and their revision commit together. A failed transaction leaves both the durable history and the view's committed index unchanged.

The remote host similarly stores ordered blocks separately from session metadata. Its saved block values are immutable; a changed block receives a new value and sync revision. Metadata, block changes, event records, and command receipts share one SQLite transaction. A full history is reconstructed when requested, while unchanged polling reads only revision metadata.

The host reads both complete JSON snapshots and normalized block storage, converting a legacy session on its next successful save. Host programs that require all blocks inside `sessions.snapshot` cannot read a converted session; do not downgrade a converted database to those programs. Desktop disconnection and reconnection do not change the host's storage owner.
