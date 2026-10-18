# Vox

Vox is vorkspace's central voice and app controller. Start it with the microphone
in the workspace title bar. It waits silently for spoken input, without a
greeting or an automatic task. It stays connected when switching chats,
projects, or Monos. Stop it with the
same button. Minimizing/hiding the document, leaving the window, a lost device,
or a failed connection releases the call.

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

The controller has its own temporary conversation. It uses the default Codex
CLI ChatGPT sign-in, gpt-live-1-codex for audio, and GPT-6.1 Sol with low reasoning
effort for delegated app actions. Target sessions and Monos retain their own
models, permissions, drafts, follow-up behavior, and execution/persistence paths.
Composer writes leave the text unsent and append by default, preserving existing
text and attachments. Explicit replacement requires the exact text previously
read; intervening typing wins. Voice transcripts are not saved by vorkspace;
messages sent to target chats are saved normally.

This first slice sends one-way messages. Acceptance does not mean the work is
finished; ask Vox to read the target's reply when needed. No background polling
or automatic completion relay is added. Stopping Vox prevents new requests;
work already accepted by a target continues under that target's ownership.

## Module boundaries

| Owner                                                 | Responsibility                                                                                |
| ----------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| `src/features/vox/ui/VoxProvider.tsx`                 | One workspace call, lazy loading, visible state, stop on hide/unmount.                        |
| `src/features/vox/ui/VoxButton.tsx`                   | Start/stop control and actionable errors.                                                     |
| `src/features/vox/model/vox.ts`                       | Microphone, WebRTC, Codex protocol, request receipts, resource cleanup.                       |
| `src/features/vox/model/protocol.ts`                  | Vox identity, silent startup and routing instructions, bounded tool contract.                 |
| `src/features/vox/model/controller.ts`                | Fresh local scope, target resolution, navigation, composer commands, explicit app capability. |
| Existing app-action handler and target session        | Message/run acceptance, provider permissions, queueing and history.                           |
| Mounted session/Mono composer and durable draft store | Live input, attachments, conflict handling and persistence.                                   |

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
  No PCM, meters, transcripts, or token-by-token output enter React.
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
title-bar replacement, tool delivery, and errors in both themes. Real
Codex/ChatGPT calls with synthetic speech routed a question to a named test Mono
and filled its test composer without sending. These used synthetic app targets
and returned non-silent speaker audio. They validate the protocol path; they are
not a latency benchmark or a physical microphone test.

The Vox startup check observed ten seconds with a silent microphone on a real
Codex/ChatGPT connection: received audio energy was zero, with no backing-agent
turns or delegated app actions. Subsequent synthetic speech successfully sent the question
to the named test Mono and produced audible reply audio. Silence is requested in
the voice model's startup instructions, without a fake user turn, muted playback,
extra request, or startup delay in the app.

## Review (2026-10-17)

Validation passed: 5,272 web tests (13 existing live-provider tests skipped),
TypeScript, ten Chromium/WebKit browser checks, and the live silent-start and
named-Mono routing probe described above.

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
