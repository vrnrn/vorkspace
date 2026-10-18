# vox

vox is vorkspace's central voice and app controller. It is off by default.
Enable it in **Settings → Vox → General → Enable Vox**, beside “Requires Codex
with ChatGPT live voice access.” Turning it off hides the launch control and
gracefully ends any call, keeping conversation and memories.

Vox lives in Vorkspace's top-right title bar. Its compact three-glyph face is the
control: click to start or retry, then click again to end. It waits silently for
spoken input, without a greeting or automatic task. There is no separate panel,
microphone mute button, or speaker button. Switching chats, projects, or Monos
keeps the call connected; hiding or closing the workspace gracefully stops it.
Replacing or removing the title-bar control alone does not end or restart a call.
Settings retains the control when the compact global title bar is absent.

**Settings → Vox → Audio → Silent replies** mutes the current audio element and
persists for subsequent calls and app restarts. It does not change the microphone
or restart the connection. While a call is active in silent mode, only its latest
completed native assistant reply appears beneath the face in the shared glass
Popover. The bubble is 280 px wide and at most 180 px high, supports text selection
and wheel/keyboard scrolling, and leaves focus alone. Click-away or Escape hides
that bubble only; a later reply can appear again. User speech and partial text are
never displayed. Errors use a separate notice even with spoken replies enabled.
Vox's visible copy and reply display are lowercase; underlying replies and app
action payloads retain their original case. Settings keeps standard app casing.

The title-bar face uses incumbent grayscale fills, monospace glyphs, and shared
focus treatment. Stopped calls show static `vox`. Connecting, thinking, talking,
and live idle states use distinct CSS glyph sequences, including sparse idle
blinks and glances. Reduced Motion shows a static state-specific glyph. Animation
effects exist only while a call is on and the control is mounted.

vox defaults to the MacBook's built-in microphone rather than following the
system input. **Settings → Vox** lists connected microphones once, using their
native names and the existing settings layout and capitalization. Lowercase
styling belongs to the Vox control and reply bubble, not the surrounding app. The built-in default
resolves to the actual MacBook microphone without adding an alias to the menu.
The menu contains only discovered hardware inputs, without built-in or system
default aliases. A missing saved input remains named in the selector without
adding an unavailable menu entry. An older system-default preference remains
honored until an explicit device is selected. The saved app preference applies
to the next call and does not change macOS's input. A missing or unplugged
selected input stops the call with an actionable error instead of falling back
to another mic.
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

The controller has its own persistent conversation, independent of the selected
chat or project. It uses the default Codex
CLI ChatGPT sign-in, gpt-live-1-codex for audio, and GPT-6.1 Sol with low reasoning
effort for delegated app actions. Target sessions and Monos retain their own
models, permissions, drafts, follow-up behavior, and execution/persistence paths.
Composer writes leave the text unsent and append by default, preserving existing
text and attachments. Explicit replacement requires the exact text previously
read; intervening typing wins. Native conversation history retains both sides of
the voice exchange; user speech remains absent from the UI. Its lightweight
display keeps only the latest completed assistant reply. Messages sent to target
chats are saved normally.

Ending a call, helper process replacement, renderer reload, and app restart
resume the same saved native thread. A failed resume reports an error rather
than silently starting over. **Settings → Vox → Reset conversation** asks for
confirmation, ends the current call, and starts a fresh native identity on the
next call. It keeps saved memories and detaches the old history; it does not
erase that history from disk.

Vox can remember lasting facts and preferences through its own memory tools.
**Settings → Vox → Memory** uses the existing Mono memory interface to list,
add, edit, and delete them. Saved facts load on the next call and survive a
conversation reset. Settings retains the app's standard layout and casing.

This first slice sends one-way messages. Acceptance does not mean the work is
finished; ask Vox to read the target's reply when needed. No background polling
or automatic completion relay is added. Stopping Vox prevents new requests;
work already accepted by a target continues under that target's ownership.

## Module boundaries

| Owner                                                 | Responsibility                                                                                 |
| ----------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `src/features/vox/model/availability.ts`              | Default-off persisted availability and local/cross-window preference notifications. |
| `src/features/vox/model/replyMode.ts`                 | Persisted Silent replies preference and change notifications. |
| `src/features/vox/ui/VoxProvider.tsx`                 | Workspace call, microphone/WebRTC/helper ownership, preferences, graceful stop. |
| `src/features/vox/ui/VoxButton.tsx`                   | Title-bar start/end control, shared reply bubble, and error notice. |
| `src/features/vox/ui/VoxFace.tsx` and `ui/vox.css`      | State glyph sequences and reduced-motion fallback. |
| `src/features/vox/model/display.ts`                   | Bounded completed-reply authority; immediate notifications to mounted controls and coarse voice activity.            |
| `src/features/vox/model/expressions.ts`               | Lowercase face IDs, emotions, and connection/activity priority.                                |
| `src/features/vox/model/vox.ts`                       | Microphone, WebRTC, Codex protocol, request receipts, resource cleanup.                        |
| `src/features/vox/model/conversation.ts`              | Revision-guarded persistent native thread identity and reset.                                  |
| `src/features/vox/model/context.ts`                   | Bounded voice context from the same native conversation only.                                  |
| `src/features/vox/model/memory.ts`                    | Vox-owned memory through existing Mono file and memory operations.                             |
| `src/features/vox/model/microphone.ts`                | Saved input selection, exact-device capture, permission bootstrap, and missing-input errors.   |
| `src/features/vox/model/protocol.ts`                  | Vox identity, silent startup and routing instructions, bounded tool contract.                  |
| `src/features/vox/model/controller.ts`                | Fresh local scope, target resolution, navigation, composer commands, explicit app capability.  |
| Existing app-action handler and target session        | Message/run acceptance, provider permissions, queueing and history.                            |
| Mounted session/Mono composer and durable draft store | Live input, attachments, conflict handling and persistence.                                    |

The app shell supplies navigation and existing app-action callbacks. It does not
own Vox's target-selection policy. Composer editing stays in the sessions
feature because the editor, including its restored attachments, owns the draft.
No second bot record, target transcript, draft store or global audio state is
introduced for Vox.

## Ownership and cost

- The workspace provider owns one call independently of the mounted title-bar
  control. Removing the control removes its visual effects and display observer,
  while the call continues. Hiding or closing the workspace stops it gracefully.
  A native harness reservation admits one Vox writer across app windows,
  protecting its persistent conversation. Transport loads only after starting.
- Availability defaults off. Disabling ends the call and hides the control
  without remounting workspace content or resetting durable conversation and
  memory. Enabling alone does not capture or connect.
- CSS sequences animate glyph visibility without an animation clock, React frame
  updates, audio meter, PCM processing, or worker. Coarse live activity comes
  from the observed WebRTC `output_transcript.added`, `turn.created`, and
  `turn.done` events. The activity store keeps identities and timestamps, not
  those events' speech text. Old completion cannot stop a newer reply, and
  backing-agent completion cannot override ongoing voice output.
- Codex app-server owns authentication, voice delegation, and speaking its
  backing agent's result. WebRTC owns microphone encoding and speaker playback.
  No PCM, meters, or backing-agent token stream enters React. Displayed replies
  come only from native V3 `thread/realtime/transcript/done` assistant events,
  verified with Codex 0.163.0. User text is excluded from the display; all transcript
  deltas are suppressed before stdout/IPC. There is no second recognizer,
  transcription request, or reconstructed text from backing-agent output.
- The reply store publishes current reads and notifications immediately, with
  no notification timer or per-word partial subscription. Only a mounted title-bar control
  subscribes. Retention is one completed assistant reply, capped at 4,000 UTF-16
  characters with explicit truncation. Completion replaces the previous reply;
  the bubble renders it only during an active call in silent mode. Ending a call retains this temporary
  value in memory; starting a new call clears it. This display projection is
  separate from durable native conversation history.
- Codex stores the conversation in the private Mono store using the default
  account. The saved thread identity is revision guarded so reset or replacement
  cannot publish an obsolete identity. Recent voice context reads at most four
  turns from that thread and projects at most 3,000 UTF-16 characters into native
  `initialItems`. `includeStartupContext:false` avoids a generic workspace scan
  and unrelated or retired thread context. Full backing-agent history remains
  retained.
- Stopping requests native transcript-tail flushing and waits for
  `thread/realtime/closed` before killing the helper. Ending releases media before waiting for persistence. Disabling Vox, normal window
  close, and app Quit wait for graceful shutdown. A failed flush is reported; a force kill or
  crash may lose speech that has not yet flushed.
- Saved facts reuse Mono file storage under the reserved `vox` owner, including
  its budgets, compare-and-swap writes, secret filters, archive, and search.
  `memory.read/search/add/replace/remove` target only Vox files. Loading saved
  facts on the next call adds no polling, recognizer, or speech service.
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
  thinking/error states take priority over model moods. CSS sequences reflect
  connecting, thinking, talking, and idle activity; Reduced Motion disables them.
- The backing agent gets one dynamic `vorkspace` tool. An explicit in-process
  capability extends the existing app-action handler across known local
  projects and allows messages to Monos as the user. Normal CLI callers keep
  their existing project and Mono restrictions. One Codex app-server helper runs
  per call; no second controller daemon, model download, or new credential store
  is introduced.
- The helper process is per-call; its native conversation is persistent. Its
  shell sandbox is read-only, with command/edit approvals
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
reply text only in silent mode, no user speech, shared bubble placement and
dismissal, long-reply scrolling, latest-reply replacement, title-bar start/end,
glyph visibility, animation removal when stopped/unmounted, Reduced Motion, and
persisted silent-mode changes on one connection. Microphone tests
cover saved preferences across reload, next-call exact-device constraints,
missing inputs, cancellation, permission cleanup, and unplugging an active input.
Store tests cover final-only replies, user text excluded from display, bounded
retention, immediate reads/notifications, and idle cost. Conversation tests cover
resume, reset, stale identity publication, same-thread context, native writer
reservation, and memory ownership. Settings checks include memory editing and
reset errors beside the Conversation action. Real
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

A production-code WebRTC probe resumed the same conversation after End, a new
helper process, and renderer reload, recalling “cobalt lantern”. Reset created a
fresh native thread with empty initial context. All three calls stayed silent
before input. This isolated integration used synthetic speech and targets.

## Review (2026-10-17)

The original controller validation passed 5,272 web tests (13 existing
live-provider tests skipped), TypeScript, ten Chromium/WebKit checks, and the
live silent-start and named-Mono routing probe. Current conversation/memory
validation passed 5,307 web tests (13 skipped),
18 Chromium/WebKit checks, two native cross-window reservation tests, and
Clippy. These are historical conversation/memory results. Current title-bar
validation passed 133 focused tests, TypeScript, 24 Chromium/WebKit checks and
two long-reply rechecks. Native same-window start/end and the silent switch were
verified. An isolated synthetic-speech probe observed silent startup without
app actions and reply activity returning to listening. Current screenshots and
detailed results are recorded in `design-qa.md`. The MR checkpoint also passed
the full web suite (5,313 tests, 13 skipped), native formatting and Clippy, and
662 native tests (one ignored). These checks do not establish speed, battery,
or memory improvements.

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
