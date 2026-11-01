# Repository map

The desktop is a Rust workspace that runs as one process. The remote host is a separate Node application. Directory boundaries follow those runtime owners, and dependencies only point downward. That holds between crates and between the top-level modules inside each crate, and `scripts/checks/module-cycles.test.mjs` enforces it. A lower module that needs something from above receives it as a small trait, such as `BotsHost`, `VoxHost`, `AnnotationDraft`, or `SelectableText`. The one permitted cycle is `agent` and `bot_model`, because Bot records hold their settings and turn settings hold the Bot.

| Path | Responsibility |
| --- | --- |
| `crates/vorkspace-core/` | Provider processes, durable sessions, files/Git, Bots, Channels, Vox control, Copilot gateway, and remote transports |
| `crates/vorkspace-gpui/` | The application: window shell, conversations, transcript, Bots, settings, inbox, and native input |
| `crates/vorkspace-ui/` | UI foundation: preferences, theme and palettes, the animation clock, motion, and small shared marks |
| `crates/vorkspace-runtime/` | Session execution, remote session binding, and persistence coalescing |
| `crates/vorkspace-editor/` | Editor analysis (bounded QuickJS) and native syntax (tree-sitter grammars) |
| `crates/vorkspace-model/` | Plain data models: pane layout, Markdown elements, Git graph and checkout cache |
| `crates/vorkspace-macos/` | AppKit integrations, hotkeys, updater, relaunch, and hang diagnostics |
| `host/` | Remote host service, authentication, SQLite store, session engine, SSH deployment package, and tests |
| `embedded/editor/` | Editor parsing, diagnostics, completion, search, formatting, and syntax palettes for bounded QuickJS workers |
| `embedded/vox/` | Microphone and WebRTC audio page; Rust admits actions and owns the call |
| `scripts/desktop/` | Development runner, build, bundle, signing, and bundle configuration |
| `scripts/release/` | Version stamping and signed update archive tools |
| `scripts/checks/` | Repository/dependency validation and disposable SSH integration checks |
| `scripts/benchmarks/` | Reproducible synthetic measurements with isolated data |
| `scripts/assets/` | Icon generation tools, run by hand when an upstream icon set changes |
| `crates/vorkspace-gpui/fixtures/` | Visual SDK sample programs embedded as development fixtures |
| `vendor/` | The project's own GPUI fork for macOS: framework, Metal renderer, and platform layer. It no longer tracks upstream, so fix GPUI problems here directly |
| `licenses/` | Third-party license texts included in the app bundle |
| `docs/` and `docs/eli5/` | Technical guides and matching plain-English companions |

## Runtime modules

In the core, `store` owns SQLite and sidecar persistence; `provider` owns CLI transports and process lifetime; `transcript` projects bounded display data. `bot` owns Bot and Channel state, `voice` owns Vox, and `remote` owns authenticated remote transport. Keep durable writes at those boundaries.

In GPUI, `conversation` connects the session runtime to input and transcript views. `shell` owns window navigation and workspace surfaces, with its topics in `shell/`. `git_tools/` holds source control surfaces, `file_tools/` the explorer and editor pane, and `composer/` prompt entry. `terminal`, `settings`, and `bots` contain their native views. Rendering consumes core state; it does not create a second persistence authority.

The host's top-level modules own server, service, store, engine, and sync lifecycles. `host/runtime/models/` contains domain records and validation; `providers/` contains provider-specific adapters; `protocol/` contains their shared stream and RPC machinery; `platform/` owns Node filesystem/platform helpers; `lib/` contains small pure utilities. These modules run on the remote machine.

## Sources, generated files, and tests

Edit embedded helpers in `embedded/`, then run `npm run assets`. Their committed outputs belong to the consuming crate's `assets/` directory and allow Cargo builds without npm. `build/` and `target/` contain disposable local outputs. Do not put application sources or user data there.

Rust tests live in a `tests.rs` beside their module, or inline when short. Test helpers that cross a crate boundary sit behind that crate's `test-support` feature. Host and audio tests sit next to their JavaScript owners. Host-only test clients live in `host/test-support/`, outside the shipped runtime modules. Tool tests live beside scripts or under `scripts/checks/`. [Testing](testing.md) lists checks and the real-device boundaries they do not cover.

Plans, measurements, QA reports, and other point-in-time work records stay outside the repository. Documentation describes supported behavior and current commands, with an ELI5 companion for every technical page.
