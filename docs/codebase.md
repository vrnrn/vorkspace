# Codebase Memory

Codebase Memory is an optional local code index for a project and its Git working copies. It is off by default. Open the project's settings menu and enable **Codebase Memory** to add **Codebase** to the sidebar. Remote projects are not supported. Opening Codebase starts or resumes that working copy's index in the background; each working copy has its own tab and index identity. The normal workspace tiling limits apply.

The native page searches symbols, shows bounded source excerpts, and lists direct callers and callees. Its relationship diagram supports the existing expand and zoom controls. At narrow pane widths the symbol list and detail become separate views with a Back button. Search initially shows up to 100 matches; refine the query for more specific results. Source excerpts show up to 200 lines, relationship lists up to 50 entries, and the diagram up to 12 relationships. These are navigation previews, not a complete account of every call.

## Engine and storage

The core service in `crates/vorkspace-core/src/codebase` owns opt-in state, installation, subprocesses, cancellation and queries. GPUI presents that service; bots and Operator sessions use it through the authenticated app CLI.

First use downloads the architecture-specific macOS release of [codebase-memory-mcp v0.12.1](https://github.com/DeusData/codebase-memory-mcp/releases/tag/v0.12.1), about 48 MB compressed and 335 MB installed. The app checks the pinned byte count and SHA-256 before extracting only the executable, license and third-party notices. It never executes the upstream installer. Component installation uses owner-private staging, flushed files and an atomic directory rename. The version/checksum marker is written before that directory becomes visible. The restricted tool schemas retain their upstream MIT notice in `crates/vorkspace-core/src/codebase/LICENSE-upstream`.

The component and indexes live in `codebase/` under the selected local app data directory. Project enablement lives in the `codebase-enabled-projects` store setting. The engine's Unix sockets and coordination locks use a stable private directory at `/tmp/vcbm-<hash>`, keyed by the canonical cache directory. This short 128-bit SHA-256 prefix fits macOS socket-path limits. The app retains the directory across client replacement and restarts so a retiring daemon and its successor use the same lifetime and project locks; it does not delete these locks when a frontend exits. It does not change external provider MCP configuration, shell hooks, PATH, or an existing Codebase Memory installation.

The engine runs locally through stdio with its HTTP interface disabled before startup. Indexing and browsing in the native page do not upload source to a model provider. When an agent requests code through the app CLI, returned excerpts enter that agent’s normal provider conversation under its existing access policy. Optional semantic embeddings are disabled; full indexing still builds the engine's symbol and relationship graph. Each working copy has an isolated engine with two indexing workers and a 512 MiB indexing memory budget, with retained-source budgets of 64 MiB total and 8 MiB per file. Files larger than 16 MiB are excluded. The native page reports partial coverage rather than claiming every file was understood. Index status queries expose skipped, partial, unusable and unindexed file details.

## Ownership and cancellation

Up to eight working copies can have active or retiring engines. After a stdio frontend exits, its slot remains reserved until exclusive acquisition of the pinned engine's version-cohort lifetime lock confirms that detached daemon cleanup has finished. This check is nonblocking. If startup fails before the daemon handshake can be confirmed, its reservation remains conservative; retrying the same working copy retains that reservation until a successful handshake establishes the cleanup barrier. A client remains alive while its project is enabled, including when its tab closes, so the engine can watch file changes. Closing a pane does not disable the project. Disabling cancels that project's clients and watcher subscriptions while retaining its index files. A restored disabled pane cannot start an engine. Enabled projects are activated when opened or explicitly indexed; loading saved settings alone does not download or start anything.

Index requests are synchronous inside a cancellable background worker, not upstream detached asynchronous jobs. Closing a client's stdio connection cancels its request-owned work. A replacement joins its predecessor's frontend and uses the same daemon coordination locks. Disable does not wait in the query queue, and canceled workers cannot publish results. App shutdown cancels and joins these clients before general child-process cleanup; detached engines complete their upstream cleanup without blocking the app on a stuck daemon.

`CBM_ALLOWED_ROOT` alone is not read isolation. Vorkspace isolates each canonical root in a private engine cache and runtime, using the full SHA-256 of that root for the cache directory name. This also separates upstream project names that collide when path punctuation is normalized, while preserving automatic watching. A second path alias cannot start another daemon for a canonical root while its previous owner is active or stopping. Retargeting a path to a different canonical root also requires the previous owner to stop; its capacity cannot be reused for a different cache while cleanup remains outstanding. The app supplies the exact index name on every allowed query, verifies its canonical root after indexing, rejects cross-project selectors, validates relative paths and existing symlink prefixes, and omits mutating and cross-project tools. A linked working copy must belong to the selected project's Git worktree list. Each client has a two-request queue, a 32 KiB request limit and a 512 KiB response limit. Each engine query has a 30-second execution deadline; initial indexing has a 30-minute limit. Disabled, busy and failed clients return explicit errors.

## App CLI

Use the `app` command supplied to an active Bot or `/operator` turn. Regular Operator sessions use their own working copy, independent of the focused UI pane. Bots can choose one of their assigned projects and optionally one of its indexed working copies. Remote roots are rejected.

```sh
app codebase.configure --json '{"enabled":true}'
app codebase.status --json '{}'
app codebase.index --json '{}'
app codebase.tools --json '{}'
app codebase.query --json '{"tool":"search_graph","arguments":{"query":"prepare_release","limit":20}}'
app codebase.query --json '{"tool":"index_status","arguments":{"diagnostics":"full"}}'
app workspace.open --json '{"kind":"codebase"}'
```

`configure` persists the setting and starts preparation when enabled. `index` requests a fresh index and returns immediately; check `status` for completion. `tools` returns the restricted argument schemas. `query` supports symbol/source search, Cypher reads, bounded traces, source snippets, file outlines, graph schema, architecture, index status/coverage, and change detection. It rejects unsupported fields rather than silently widening the scope. Default output is JSON. Traversals are capped at five hops, result rows at 200 and output-token hints at 16,000, with stricter per-tool bounds where applicable.

`status`, `tools` and `query` are read-only actions available to read-only Operator sessions and Minions. Configuration and indexing require write authority and are unavailable to Minions. Normal app command receipts apply to writes. See [agent access](agent-access.md) for capability and request-ID rules.
