# Executable plugins

Vorkspace loads local executable plugins. `vorkspace-plugin` defines protocol version 1 and a small Rust SDK; a plugin can use any language that reads and writes the same newline-delimited JSON. GitHub, GitLab, Jira and Meetings ship as separate binaries in `Contents/PlugIns/`. The development runner and release builder build them with the desktop.

## Ownership and trust

The desktop owns navigation, the WebKit host, installed binary identities and native capability grants. Each plugin owns its commands and private documents. A call starts a process in its own process group, sends `initialize`, sends one command, then disposes the group. Keep reusable state in the supplied `data_dir`; process memory is not persistent. Calls have bounded frames (8 MiB), admission (eight concurrent processes) and deadlines. Timed-out or disconnected calls are never replayed automatically: an external action may already have happened.

Select **Plugins → Install plugin…** to install an executable you trust. Installation runs discovery against a private copy, validates the manifest, copies that exact binary into app data, and records its SHA-256. A changed installed binary requires reinstalling. Custom identities cannot replace bundled plugins. Disabling a plugin keeps its documents. Custom plugins must provide `ui: true`; the native Inbox's typed connector contract is currently reserved for the three bundled connector identities. A custom integration supplies its own interface and backend commands.

Native binaries run as your macOS user. They are not operating-system sandboxes and may use files, networking or subprocesses directly. Capability grants limit access through Vorkspace's bridge; they do not restrict the binary's own operating-system access. App and visual IPC tokens are removed from plugin process environments. A packaged native helper also verifies its socket peer is the desktop executable from its own app bundle; a plugin cannot impersonate that client. Direct Cargo development binaries retain the owner-only socket policy.

Bundled connectors retain the existing connector configuration directory and credentials. Their app-side `inbox::Service` is an RPC client; `inbox::BuiltinService` executes the shared adapters inside the connector binaries. Existing Inbox actions, filters, connection settings, diffs and comment receipts retain their types.

## Protocol

Standard output contains only one JSON object per line. Use standard error for diagnostics; the desktop drains it without persisting plugin output. Each request and response uses JSON-RPC 2.0 with a numeric ID.

The host starts a process and sends:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"data_dir":"/private/plugin/data","cwd":"/selected/project"}}
```

The result is the manifest:

```json
{"jsonrpc":"2.0","id":1,"result":{"protocol_version":1,"id":"example.hello","name":"Hello","version":"1.0.0","capabilities":[],"ui":true,"connector":false}}
```

The host then sends a command such as `plugin.describe` or `ui.document`. Respond with the matching ID and either `result` or `error: {"code": -32000, "message": "…"}`. `plugin.describe` returns the manifest. `ui.document` returns `{"html":"…"}`. Other method names and parameter shapes belong to the plugin.

A backend can call the host while processing a command: emit a request whose method is prefixed with `host.`, read its correlated response, then finish the original command. The SDK's `Host::call` handles this exchange. `initialize` cannot request capabilities before its manifest has been validated.

## Arbitrary interfaces

`ui.document` can return any inline HTML, CSS and JavaScript. It runs inside a nonpersistent WebKit view in an opaque `sandbox="allow-scripts"` iframe. It has no file base URL, parent DOM access, app credentials or arbitrary application IPC. Navigation is guarded; ordinary HTTP(S) links open externally. Network fetches from the interface are blocked; perform service operations in the executable and call them from the interface. HTTPS scripts/styles and image assets follow the existing HTML host's content policy.

The host injects this asynchronous API:

```js
const value = await vorkspace.call("hello", { name: "World" });
const providers = await vorkspace.host("ai.models");
```

`call` sends a command to the plugin's executable. `host` requests a declared native capability. The bridge validates the originating opaque child, main-frame forwarding nonce, installed identity, enabled state and capability. Generated chat artifacts keep their existing bridge-free host.

CSS variables include `--background`, `--surface`, `--foreground`, `--muted`, `--border`, `--accent`, and `--link`. Theme changes update variables without replacing the document. Listen for `vorkspace-visibility` and read `event.detail` to pause optional work while hidden. Vorkspace retains the document when switching app views.

## Native capabilities

| Manifest grant | Methods | Contract |
| --- | --- | --- |
| `ai.text` | `ai.models`, `ai.generate` | Existing visible providers and selected accounts; text-only provider work without Vorkspace tools or write permissions. |
| `audio.transcribe` | `audio.start`, `audio.stop`, `audio.status` | One microphone/system-audio recording, scoped to plugin and recording ID; source-labelled text and metadata only. |

`ai.generate` accepts `provider`, `model` and `prompt`, returning `{"text":"…"}`. The project and selected provider accounts are captured when a backend command starts; a project switch does not change an in-flight analysis. Each generation has a cancellation deadline. The plugin owns user consent, model/prompt settings and derived output.

`audio.start` accepts a new validated `id`. An accepted receipt means admission has begun, not that audio is ready. `audio.status` takes `id` and the last received sequence in `after`; it returns `state`, finalized `segments`, per-source `partials`, and `reload`. Load the recorder's durable text if `reload` is true. Native capture owns the microphone lease, language reservation, state and append-only transcript until both sources finish cleanup. `audio.stop` can stop only the caller's recording. Quit or a desktop disconnect finishes the meeting; ordinary dictation can continue independently after application Quit. See [Meetings](meetings.md).

## Build an independent plugin

The working example is `crates/vorkspace-plugin/examples/hello.rs`. It defines a manifest, returns an arbitrary HTML interface and handles a button request in Rust without importing any GUI or connector crate.

```sh
cargo build -p vorkspace-plugin --example hello
```

Install `target/debug/examples/hello` from **Plugins**. With a custom Cargo target directory or profile, use that output directory instead. An independent Rust project can use a path dependency on `crates/vorkspace-plugin`; other languages implement the protocol directly. No runtime language or source checkout is required by an installed compiled plugin.
