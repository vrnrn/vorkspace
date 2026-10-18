# vox

vox is vorkspace's central voice and app controller. Click **vox** in the
workspace title bar to start it and open its small square panel. It waits
silently for spoken input, without a greeting or an automatic task. The panel
shows a simple face and vox's completed replies; your speech is not displayed.
Close or Escape hides the panel while the call continues; reopening it retains
recent captions. **end**
stops the call. Switching chats, projects, or Monos keeps the call connected.
Minimizing/hiding the document, leaving the window, a lost device, or a failed
connection releases it.

**silent replies** mutes vox's speaker while leaving your microphone and native
text replies active. **mic on / mic off** independently controls input. These
switches reuse the existing connection. Silent replies remains selected across
calls in this workspace window; each new call starts with the mic enabled.
The panel inherits vorkspace's shared popover, theme, and icon system, using
grayscale content tones for the face and controls. Its copy and displayed
replies are lowercase; underlying replies and app-action payloads retain their
original case.

vox defaults to the MacBook's built-in microphone rather than following the
system input. **Settings → Vox** lists connected microphones once, using their
native names and the existing settings layout and capitalization. Lowercase
styling belongs to the Vox panel, not the surrounding app. The built-in default
resolves to the actual MacBook microphone without adding an alias to the menu.
The menu contains only discovered hardware inputs, without built-in or system
default aliases. A missing saved input remains named in the selector without
adding an unavailable menu entry. An older system-default preference remains
honored until an explicit device is selected. The saved app preference applies
to the next call and does not change macOS's input. A missing or unplugged selected input
stops the call with an actionable error instead of falling back to another mic.
Opening Settings only lists devices; its permission button captures briefly
only when clicked. On a first call with hidden device labels, vox may briefly
request permission, stop that temporary stream, and then capture the exact
selected device before starting WebRTC and its helper.

Try “Tell Helper to review the latest change”, “Ask the session called tests
to fix its failures”, “Put ‘review the latest change’ in Helper's composer without
sending”, or “Read the last reply in that session”. The controller
looks up actual targets and fetches recent history only when needed. It can
list, read, send to, start, and stop sessions in known local projects, and list,
read, and message Monos by name. It can also open those chats and read or fill
their composers. It asks for clarification when a target is ambiguous.

“Ask Helper what it is working on” sends that question to Helper rather than
answering it directly. The live voice prompt includes a bounded list of current
Mono names at call startup; the backing agent resolves targets against the
current roster before acting. Duplicate names require clarification or an ID.
Questions about the current chat, history, status, or available sessions also
delegate to that agent for a fresh lookup instead of guessing visible context.

The controller has its own temporary conversation. It uses the default Codex
CLI ChatGPT sign-in, gpt-live-1-codex for audio, and GPT-6.1 Sol with low reasoning
effort for delegated app actions. Target sessions and Monos retain their own
models, permissions, drafts, follow-up behavior, and execution/persistence paths.
Composer writes leave the text unsent and append by default, preserving existing
text and attachments. Explicit replacement requires the exact text previously
read; intervening typing wins. Completed assistant replies are held only in bounded
memory and are not saved by vorkspace; user transcripts are discarded. Messages
sent to target chats are saved normally.

This first slice sends one-way messages. Acceptance does not mean the work is
finished; ask Vox to read the target's reply when needed. No background polling
or automatic completion relay is added. Stopping Vox prevents new requests;
work already accepted by a target continues under that target's ownership.

## Module boundaries

| Owner                                                 | Responsibility                                                                                    |
| ----------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `src/features/vox/ui/VoxProvider.tsx`                 | One workspace call, lazy loading, mic/speaker preferences, stop on hide/unmount.                  |
| `src/features/vox/ui/VoxButton.tsx`                   | Title-bar trigger and shared anchored popover; panel visibility does not own the call.            |
| `src/features/vox/ui/VoxPanel.tsx`                    | Grayscale face, completed native replies, controls, accessible status, and follow-tail scrolling. |
| `src/features/vox/model/display.ts`                   | Bounded completed-reply authority; immediate notifications only to a mounted panel.               |
| `src/features/vox/model/expressions.ts`               | Lowercase face IDs, emotions, and connection/activity priority.                                   |
| `src/features/vox/model/vox.ts`                       | Microphone, WebRTC, Codex protocol, request receipts, resource cleanup.                           |
| `src/features/vox/model/microphone.ts`                | Saved input selection, exact-device capture, permission bootstrap, and missing-input errors.      |
| `src/features/vox/model/protocol.ts`                  | Vox identity, silent startup and routing instructions, bounded tool contract.                     |
| `src/features/vox/model/controller.ts`                | Fresh local scope, target resolution, navigation, composer commands, explicit app capability.     |
| Existing app-action handler and target session        | Message/run acceptance, provider permissions, queueing and history.                               |
| Mounted session/Mono composer and durable draft store | Live input, attachments, conflict handling and persistence.                                       |

The app shell supplies navigation and existing app-action callbacks. It does not
own Vox's target-selection policy. Composer editing stays in the sessions
feature because the editor, including its restored attachments, owns the draft.
No second bot record, target transcript, draft store or global audio state is
introduced for Vox.

## Ownership and cost

- The workspace owns one call per renderer, independently of the title-bar
  control and the selected chat. Transport code loads only after a click.
- Codex app-server owns authentication, voice delegation, and speaking its
  backing agent's result. WebRTC owns microphone encoding and speaker playback.
  No PCM, meters, or backing-agent token stream enters React. Displayed replies
  come only from native V3 `thread/realtime/transcript/done` assistant events,
  verified with Codex 0.163.0. User transcripts are discarded and all transcript
  deltas are suppressed before stdout/IPC. There is no second recognizer,
  transcription request, or reconstructed text from backing-agent output.
- The reply store publishes current reads and notifications immediately, with
  no notification timer. Only the open panel subscribes. Retention is bounded
  to 12 parts, 4,000 UTF-16 characters per part and 16,000 in total, with explicit
  truncation. Ending a call retains recent replies; starting a new one clears
  them. These replies are a temporary display, not durable chat history.
- The microphone owner resolves the saved selection before call setup and pins
  explicit devices with an exact device ID. Cancelled device lookup or permission
  completion cannot open a later capture. Unplugging the active input stops the
  call; an unavailable selection requires the user to choose an input again.
- Silent replies changes the native audio element's `muted` property. The
  service still generates and transports its normal voice response. Mic input
  uses the existing audio track's `enabled` property. Neither switch creates
  another helper, negotiates another connection, or changes speech services.
- Sixteen faces are defined centrally with identifiers and emotions. Eleven
  moods can accompany an existing app tool call through an optional expression
  field, without extra calls just for animation. Connection, mic-off, and
  thinking/error states take priority over model moods. A short face transition
  respects reduced-motion preferences; no continuous animation loop runs.
- The backing agent gets one dynamic `vorkspace` tool. An explicit in-process
  capability extends the existing app-action handler across known local
  projects and allows messages to Monos as the user. Normal CLI callers keep
  their existing project and Mono restrictions. One Codex app-server helper runs
  per call; no second controller daemon, model download, or new credential store
  is introduced.
- The voice helper is ephemeral and read-only, with command/edit approvals
  declined. It is instructed to use the app tool and leave coding to targets.
  Its messages never run through a target's provider connection.
- Tool calls validate the owning thread, action, and lifecycle. Before dispatch,
  the app rechecks whether Vox is still active. Duplicate call IDs reuse
  receipts, including failures. Changed arguments under a reused ID fail.
  At most 128 calls are admitted per connection, with request signatures capped
  at 64,000 characters and cached responses at 48,000 characters each.
- Chat history stays behind the existing bounded read/page APIs. No idle history
  serialization, extra session subscription, audio render loop, or polling runs.
- Composer control opens the target and waits for its mounted editor and normal
  draft hydration. That editor owns the edit and existing durable save path;
  there is no second draft store. Waiting is notification based, bounded to five
  seconds, and cancelled when Vox stops. Reads return at most 12,000 characters
  with a truncation flag; a truncated read cannot authorize full replacement.

This is a local desktop integration with remote ChatGPT voice, not offline speech
recognition. It requires an installed Codex with experimental V3 realtime and
dynamic tool support (verified with 0.163.0), ChatGPT voice access, and microphone
permission. The macOS app includes the usage description and audio entitlement.

## Verification

Unit tests control microphone/spawn completion, overlap, late SDP, startup
timeouts, tool routing, duplicate delivery, failed actions, stop/replacement,
cross-project reads/sends, named Mono resolution, composer ownership, stale
replacement, cancellation, and explicit controller versus ordinary CLI scope.
Browser tests exercise the real title bar and composer in Chromium and WebKit:
session and Mono draft editing without sending, draft preservation, navigation,
title-bar replacement, tool delivery, errors in both themes, completed assistant
replies without user speech, square panel geometry, retained replies after
closing, and independent mic/silent controls on one connection. Microphone tests
cover saved preferences across reload, next-call exact-device constraints,
missing inputs, cancellation, permission cleanup, and unplugging an active input.
Store tests cover final-only replies, discarded user transcripts, bounded
retention, immediate reads/notifications, and idle cost. Real
Codex/ChatGPT calls with synthetic speech routed a question to a named test Mono
and filled its test composer without sending. These used synthetic app targets
and returned non-silent speaker audio. They validate the protocol path; they are
not a latency benchmark or a physical microphone test.

The Vox startup check observed ten seconds with a silent microphone on a real
Codex/ChatGPT connection: received audio energy was zero, with no backing-agent
turns or delegated app actions. Subsequent synthetic speech successfully sent the question
to the named test Mono and produced audible reply audio. Silence is requested in
the voice model's startup instructions, without a fake user turn, muted playback,
extra request, or startup delay in the app. A subsequent real call with the panel
reply stream enabled received native transcript events and
routed synthetic speech to a named test Mono. With silent replies enabled, the
audio element stayed muted and its mic track stayed enabled. Audio transport
continued, as expected. These calls use synthetic app targets rather than real
user chats. A later isolated macOS WKWebView probe with synthetic speech received
a completed hello reply and a second reply. A live Chromium probe completed a
backing-agent turn and returned a final answer from synthetic current-chat
history. These observations do not establish the cause of the reported input
stall or compare physical AirPods and MacBook microphones. The built-in default
addresses the suspected input route; no end-to-end latency benchmark was run.

## Review (2026-10-17)

The original controller validation passed 5,272 web tests (13 existing
live-provider tests skipped), TypeScript, ten Chromium/WebKit checks, and the
live silent-start and named-Mono routing probe. The panel adds the caption,
expression, and playback-control checks described above; its current results
are recorded in `design-qa.md`.

- Traced startup through microphone permission, helper spawn, thread creation,
  SDP, connection, delegation, app acceptance and cleanup. The old preview naming
  and app-shell routing are consolidated under Vox. Startup now explicitly waits
  for input instead of leaving greeting behavior unspecified.
- Reviewed navigation during a pending operation, stop during lookup/hydration,
  late microphone/spawn completion, old-owner cleanup, duplicate tool delivery,
  failed mutations, ambiguous names and manual edits. Tests cover pinned targets
  and reject stale effects before dispatch; accepted target work retains its own
  lifecycle after Vox stops.
- Reviewed authority and storage: native CLI events cannot opt into Vox's private
  in-process capability. Target sessions retain their normal permission and send
  paths. Composer controls use the mounted editor after hydration, append by
  default and require an exact prior read for replacement.
- Reviewed interactive cost: transport stays lazy, startup metadata and receipts
  are bounded, history is fetched on demand, and audio remains in WebRTC. No
  background discovery polling or high-frequency React audio state is added.
- Remaining limits: live model routing and silent-start behavior depend on the
  remote service; tests demonstrate observed behavior rather than a deterministic
  server guarantee. Delivery receipts mean acceptance, not completed work or
  exactly-once external execution. Physical microphone/device switching and
  whole-app latency/memory budgets have not been measured.

```sh
NODE_OPTIONS=--no-experimental-webstorage npm run check:web
npx playwright test tests/browser/vox.spec.ts
npm run tauri -- build --target aarch64-apple-darwin --bundles app \
  --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

The Node option keeps Node 26 web storage from interfering with the test DOM.
The build command creates an app bundle without an updater signature or DMG.
