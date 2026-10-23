# Vox

Vox is an optional voice conversation in the title bar. Enable it in Settings → Vox; it starts disabled. Clicking its face begins a call. Stop releases microphone capture immediately, ends the call, and denies late app actions. Silent mode keeps the reply bubble while suppressing spoken playback.

Hiding the application or minimizing the calling window ends its Vox call. Switching chats, projects, or Settings, losing keyboard focus, and covering the window keep the call connected. Native hide/minimize observers belong to the active call and are removed when it ends; a delayed event from an earlier call cannot stop its successor.

The silent reply is selectable and scrollable. Escape, clicking outside, or Dismiss hides that completed reply until a new reply arrives; activity changes do not reopen it. Display retains at most 4,000 UTF-16 units and shows an ellipsis when truncated. Resume context is limited to four recent turns and 3,000 UTF-16 units, without changing the provider's stored history.

Microphone capture uses the selected device exactly, or a uniquely matching label after WebKit rotates device IDs. The initial default prefers the built-in MacBook microphone. Capture never silently switches to another device when a selection disappears.

Vox can inspect and control supported local sessions and Bots through native handlers. It inherits the chosen provider account and model when creating a session. Ambiguous project names require a full path. Composer updates check revisions and preserve attachments and unsent user input; follow-ups use the durable queue.

The saved conversation uses the provider's native thread identity. Reset increments its revision so a late reply cannot restore a cleared thread. Memory belongs to the reserved `bots/vox` folder and is opened with the native conflict-aware editor.

## Runtime

`crates/vorkspace-core/src/voice/runtime.rs` owns the Codex app-server child, bounded JSON-RPC channels, exclusive call lock, tool receipts, and shutdown. It saves the accepted native thread identity with a revision check before opening audio. Resume failure never starts a replacement conversation automatically. Tool calls must match that thread and the embedded action schema; repeated call IDs reuse the prior result without repeating effects.

`crates/vorkspace-gpui/src/vox/` owns the controls, ephemeral WebKit surface, and microphone permissions. `crates/vorkspace-gpui/src/shell/vox.rs` dispatches actions through existing conversation and Bot owners. `crates/vorkspace-core/src/voice.rs` owns durable identity and memory operations.

`embedded/vox/` builds the small WebRTC audio page embedded in the core. It exchanges SDP, microphone devices, and playback activity with a per-call capability-protected loopback endpoint. It cannot dispatch workspace tools or launch processes. WebRTC preserves the Codex subscription sign-in path and browser echo cancellation; the native PCM transport requires API-key authentication. No Node voice controller is started.

End, window disposal, and audio failures ask Codex to stop realtime and flush the transcript tail. The controller waits for the matching closed notification before releasing the helper; it reports uncertainty if closure cannot be confirmed. Shutdown does not replay accepted effects.

The private page uses an unpredictable URL, exact Host/Origin validation, bounded request and event queues, and scoped native commands. It has no general application IPC. Closing or stopping retains enough time to flush the conversation identity before helper termination.

Run `npm run test:voice` for microphone and audio-page tests, and `npm run test:rust` for native lifecycle/persistence coverage. Physical microphone, WebRTC audio, and authenticated provider flows need live validation.
