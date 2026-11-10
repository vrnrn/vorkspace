# Repository map

The desktop is a Rust workspace that runs as the app plus a small helper process for clipboard history, dictation, and meeting audio. The remote host is a separate Node application. Directory boundaries follow those runtime owners, and dependencies only point downward. That holds between crates and between the top-level modules inside each crate, and `scripts/checks/module-cycles.test.mjs` enforces it. A lower module that needs something from above receives it as a small trait, such as `BotsHost`, `VoxHost`, `AnnotationDraft`, or `SelectableText`. The one permitted cycle is `agent` and `bot_model`, because Bot records hold their settings and turn settings hold the Bot.

| Path | Responsibility |
| --- | --- |
| `crates/vorkspace-core/` | Provider processes, durable sessions, files/Git, Bots, Vox control, Copilot gateway, and remote transports |
| `crates/vorkspace-gpui/` | The application: window shell, conversations, transcript, Bots, settings, inbox, and native input |
| `crates/vorkspace-ui/` | UI foundation: preferences, theme and palettes, the animation clock, motion, and small shared marks |
| `crates/vorkspace-runtime/` | Session execution, remote session binding, and persistence coalescing |
| `crates/vorkspace-editor/` | Editor analysis (bounded QuickJS) and native syntax (tree-sitter grammars) |
| `crates/vorkspace-terminal/` | Safe Rust owner and public C adapter for the statically linked libghostty-vt terminal engine |
| `crates/vorkspace-model/` | Plain data models: pane layout, Markdown elements, Git graph and checkout cache |
| `crates/vorkspace-helper/` | Background clipboard and dictation, native meeting capture, and helper IPC |
| `crates/vorkspace-meetings/` | Shared transcript storage, editable meeting documents, and summary generation |
| `crates/vorkspace-macos/` | AppKit integrations, hotkeys, updater, relaunch, and hang diagnostics |
| `host/` | Remote host service, authentication, SQLite store, session engine, SSH deployment package, and tests |
| `embedded/editor/` | Editor parsing, diagnostics, completion, search, formatting, and syntax palettes for bounded QuickJS workers |
| `embedded/vox/` | Microphone and WebRTC audio page; Rust admits actions and owns the call |
| `scripts/desktop/` | Development runner, build, bundle, signing, and bundle configuration |
| `scripts/release/` | Version stamping and signed update archive tools |
| `scripts/checks/` | Repository, dependency and documentation validation, and disposable SSH integration checks |
| `scripts/benchmarks/` | Reproducible synthetic measurements with isolated data |
| `scripts/assets/` | Icon generation tools, run by hand when an upstream icon set changes |
| `crates/vorkspace-gpui/fixtures/` | Visual SDK sample programs embedded as development fixtures |
| `vendor/` | The project's own GPUI fork for macOS: framework, Metal renderer, and platform layer. Also a patched `block` crate, which that layer's dependencies need to keep compiling. It no longer tracks upstream, so fix GPUI problems here directly |
| `vendor/libghostty-vt/` | Checksummed Ghostty VT source and offline Zig packages, pinned provenance and bundled license notices |
| `licenses/` | Third-party license texts included in the app bundle |
| `docs/` and `docs/eli5/` | Technical guides and matching plain-English companions |

## Runtime modules

In the core, `store` owns SQLite and sidecar persistence; `provider` owns CLI transports and process lifetime; `transcript` projects bounded display data. `bot` owns Bot state, `voice` owns Vox, and `remote` owns authenticated remote transport. Keep durable writes at those boundaries.

`vorkspace-runtime` runs without GPUI. It owns admission and persistence for Bot card refreshes. The desktop starts one writer on demand and drains its accepted saves when the app quits.

Turn updates carry an explicit outcome for completion, cancellation, or failure. Display text can change independently of follow-up scheduling. Store preference reads and writes can operate without projecting or rewriting the historical turn receipts.

In GPUI, `conversation` connects the session runtime to input and transcript views. `shell` owns window navigation and workspace surfaces, with its topics in `shell/`. `git_tools/` holds source control surfaces, `file_tools/` the explorer and editor pane, and `composer/` prompt entry. `terminal`, `settings`, and `bots` contain their native views. Rendering consumes core state; it does not create a second persistence authority.

`launch` resolves the invocation, selects the data source, and prepares persisted state before GPUI creates its first window. The entry point owns the shared quit and restart sequence, including the Bot write barrier.

`shell/app_core/` owns shared window layouts, session leases, scheduled work and settings publication. The settings writer coalesces immutable window snapshots, encodes the multi-window setting and its focused-window compatibility projection on a worker, and commits the pair together. A synchronous durability barrier uses the same writer when a window or project transition requires the layout to be saved first. `notes/` separates editing and revision-checked saves from library and document rendering.

Conversation changes carry their cause to the shell. Catalog and draft edits update the relevant summaries. Run transitions also update scheduled work and visibility. Checkout changes, settled runs, and requests that pause the active agent for input can refresh project files. The turn protocol distinguishes incremental rows, authoritative Host history, and a Bot's admitted history. Status-only updates skip transcript projection.

Each working directory has one Git observation per app store. The checkout catalog owns polling, version checks, the diff index and derived file statuses. Each window selects its own checkout and shares its immutable observation with other windows on the same directory. A mutation invalidates the checkout before its next scan publishes, and warm checkout observations share an app-wide memory budget.

The host's top-level modules own server, service, store, engine, and sync lifecycles. `host/runtime/models/` contains domain records and validation; `providers/` contains provider-specific adapters; `protocol/` contains their shared stream and RPC machinery; `platform/` owns Node filesystem/platform helpers; `lib/` contains small pure utilities. These modules run on the remote machine.

## Sources, generated files, and tests

Edit embedded helpers in `embedded/`, then run `npm run assets`. Their committed outputs belong to the consuming crate's `assets/` directory and allow Cargo builds without npm. `build/` and `target/` contain disposable local outputs. Do not put application sources or user data there.

Rust tests live in a `tests.rs` beside their module, or inline when short. Test helpers that cross a crate boundary sit behind that crate's `test-support` feature. Host and audio tests sit next to their JavaScript owners. Host-only test clients live in `host/test-support/`, outside the shipped runtime modules. Tool tests live beside scripts or under `scripts/checks/`. [Testing](testing.md) lists checks and the real-device boundaries they do not cover.

Plans, measurements, QA reports, and other point-in-time work records stay outside the repository. Documentation describes supported behavior and current commands, with an ELI5 companion for every technical page.
