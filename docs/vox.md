# Vox

Vox is an opt-in voice controller for Vorkspace sessions and Monos. It uses the
default Codex CLI ChatGPT sign-in and requires live voice access, a compatible
Codex installation, and microphone permission. Audio uses remote ChatGPT voice;
this is not offline speech recognition.

## Using Vox

Enable **Settings → Vox → General → Enable Vox**. It is off by default. Click
the face in the top-right title bar to start a call, and click again to end it.
Vox waits for spoken input without greeting you. The face animates while a call
is active and shows static `vox` when stopped; Reduced Motion uses static faces.

Calls stay connected while navigating between chats, projects, Monos, and
Settings. Hiding or closing the workspace, quitting, or disabling Vox ends the
call. Enabling Vox alone does not start a call or capture audio.

Under **Settings → Vox → Audio**:

- **Silent replies** mutes playback immediately without restarting the call.
  Only the latest completed assistant reply appears below the face. The bubble
  supports selection and scrolling; click-away or Escape dismisses it. User
  speech and partial transcripts are not displayed. Errors remain visible in
  either reply mode.
- **Default microphone** applies to the next call without changing macOS input.
  Vox defaults to the built-in MacBook microphone. The selector lists connected
  hardware inputs once. An unavailable or unplugged selected input produces an
  error rather than falling back to another microphone.

Vox's control and displayed replies use lowercase and the app's grayscale
styling. Settings retains the standard app casing and layout.

## Controlling chats

Vox can find, read, open, and message sessions and Monos, start or stop sessions
in known local projects, and read or fill their composers. For example:

- “Tell Helper to review the latest change.”
- “Ask the session called tests to fix its failures.”
- “Put ‘review the latest change’ in Helper's composer without sending.”
- “Read the last reply in that session.”

Targets are resolved against current app state. Ambiguous names require
clarification. Coding runs in the target chat with its existing model,
permissions, and execution path. Composer edits append by default and preserve
attachments; replacement requires an unchanged prior read, so intervening typing
wins. Message acceptance does not mean the task has finished. Ask Vox to read the
reply; completion is not automatically relayed.

## Conversation and memories

Vox has one persistent conversation, independent of the selected chat or project.
Later calls and app restarts resume it. Resume failures report an error rather
than silently replacing the conversation.

**Settings → Vox → Conversation → Reset conversation** ends the call and starts
a fresh conversation on the next call. It keeps saved memories and does not
erase the previous native history from disk.

Vox can save facts and preferences through its memory tools. **Settings → Vox →
Memory** lists them with the same add, edit, and delete controls as Mono memories.
Saved memories load when the next call starts and survive conversation reset.

## Implementation

| Owner                                                                                                             | Responsibility                                                                       |
| ----------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| [VoxProvider](../src/features/vox/ui/VoxProvider.tsx)                                                             | Workspace call lifecycle and preferences, independent of mounted title-bar controls. |
| [vox.ts](../src/features/vox/model/vox.ts)                                                                        | Exact microphone capture, WebRTC, Codex helper, request receipts, and shutdown.      |
| [controller.ts](../src/features/vox/model/controller.ts)                                                          | Current target resolution and delegation through existing app and composer handlers. |
| [conversation.ts](../src/features/vox/model/conversation.ts) and [memory.ts](../src/features/vox/model/memory.ts) | Saved native thread identity and Vox-owned Mono memory storage.                      |
| [display.ts](../src/features/vox/model/display.ts) and [VoxButton](../src/features/vox/ui/VoxButton.tsx)          | Latest completed native reply, coarse activity, title-bar face, and reply bubble.    |

WebRTC carries audio directly; the Codex app-server owns authentication and voice
inference. Each call uses one helper and resumes the saved native thread. The
voice model is `gpt-live-1-codex`; delegated app actions use `gpt-6.1-sol` with low
reasoning effort and a read-only shell sandbox. The integration uses Codex's V3
realtime and dynamic tools, verified with Codex 0.163.0.

The boundaries that protect continuity and keep work bounded are:

- Native admission permits one Vox writer across app windows. Saved thread
  identity is revision guarded so a stale operation cannot undo a reset.
- Shutdown releases media, requests transcript flushing, and waits for native
  closure before killing the helper. A crash or force kill may lose unflushed
  speech. Already accepted work in target chats continues independently.
- Target actions revalidate the active call before dispatch. Duplicate tool IDs
  reuse bounded receipts; changed arguments under a reused ID are rejected.
  Existing chat handlers and mounted composers own sends, drafts, and persistence.
- Conversation history stays in Codex. Startup projects at most four recent
  turns and 3,000 UTF-16 characters from that conversation into voice context.
  Saved facts reuse Mono storage under the reserved `vox` owner.
- The display retains one completed assistant reply, capped at 4,000 UTF-16
  characters with explicit truncation. Native transcript deltas are suppressed
  before IPC. Audio samples and per-word transcript updates do not enter React.
- Transport loads on demand. CSS drives face animation only while active, with
  no frame timer, audio meter, or background history polling. Silent replies
  mutes the audio element; the service still generates and transports audio.

## Checks

```sh
NODE_OPTIONS=--no-experimental-webstorage npm run check:web
npm run check:rust
npx playwright test tests/browser/vox.spec.ts
```

The Node option avoids Node 26 web storage interfering with the test DOM. Tests
cover call cancellation and replacement, named targets, composer conflicts,
conversation resume/reset, memory ownership, microphone selection, silent
replies, and title-bar lifecycle in Chromium and WebKit. Live checks use
synthetic speech and targets; they do not establish physical microphone
reliability or end-to-end performance. See [building](building.md) for local app
builds.
