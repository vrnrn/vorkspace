# Local dictation

Dictation is **off by default**, including old preferences that have no dictation
setting. In Settings → General → Clipboard & Dictation, the **Model** dropdown
chooses **Parakeet Unified English** (the default) or **Parakeet-TDT v3**. Both
are 0.6B Q8 GGUF artifacts. Selecting a model stops dictation and leaves it off;
selection alone never enables capture. If the selected artifact is missing,
Settings asks for Download or Cancel, naming that model, its approximate size
(**731 MB** Unified, **740 MB** TDT), and `~/.vorkspace/dictation/` as destination.
Cancelling keeps the newly selected model with dictation off. A verified installed
artifact is reused without a download. After changing models, choose **Local
dictation** to enable it.

Enabling also checks the selected artifact and prompts if needed. The saved
preference remains off until download and integrity verification succeed. Cancel
or dismiss the dialog to leave it off. Only that selected model is downloaded.
Use **Set up permissions** afterward. macOS must allow
Microphone and Accessibility for **vorkspace Clipboard**, the nested helper at
`vorkspace.app/Contents/Helpers/vorkspace Clipboard.app`. Denied permissions can
be changed in System Settings → Privacy & Security. Development bundles have
their own executable location; authorize that helper when testing it.

While enabled, the helper verifies and loads the selected weights once, probes
Unified's native stream with synthetic silence, and publishes **Preparing** until
that work finishes. It keeps one compatible model warm without opening the mic.
Each turn gets a new session; its scratch arenas are freed when the turn joins.
Disabling, changing models or publishing a download fences old work and joins the
worker before starting a successor. Disabled dictation never loads weights.

Press **⌘,** once to start. Amber with dots means preparation or microphone
startup; wait for the red microphone before speaking. Red and **Recording** appear
only after the prepared session and native capture owner can accept audio.
**Unified English streams stable words directly into the focused field during
capture.** Tentative hypotheses never leave the helper. Native windows advance every 560 ms
with 560 ms right context and three agreeing prefixes; a trailing partial word
waits for a whitespace boundary or finalization. Press **⌘,** again to drain
and finalize; only the remaining committed suffix is inserted. The nonactivating
notch keeps its empty left wing and microphone indicator; it displays no transcript.
**TDT v3 is finish-only**: its pinned offline artifact does not support native
streaming. It stays warm while enabled and transcribes bounded 20-second pieces
only after Stop. Settings describes the selected model's actual mode.

Starting-toggle cancels without transcription. Another toggle while finishing,
Settings Cancel, real typing, clicks/navigation, focus/app changes, input loss or
model change stops further insertion. Words already inserted remain untouched.
A cancelled stream is never replayed, even after an uncertain paste or stream
error. Recording stops after two minutes. No microphone opens while idle. Audio
is transient, never written or logged. The existing 16 kHz native capture owner
drains incremental PCM through an eight-second native buffer and a bounded
16-block worker queue (each block at most one second). Overflow fails the turn
explicitly rather than silently dropping audio. Inference/Metal work runs on the
helper's dedicated worker; Stop and cancellation do not wait for inference.

The same background helper owns clipboard history and dictation. Either feature
keeps it running after main-app Quit; disabling both stops it. Clipboard history
and dictation are independently enabled. The helper's Quit menu stops both.
The existing per-install socket, lock, supervisor and upgrade handshake are shared;
reconnecting publishes current status and configuration but never replays audio
or insertion. Settings remains available from the application menu. **⌘.** is the
Vox default; neither chord also invokes a local Settings or model action. Global
hotkeys accept only the first key-down until release.

Vox and dictation share an exclusive process lease for microphone and notch
ownership. Admission precedes showing a panel. Cancellation fences the attempt
epoch and requests cooperative inference cancellation, retaining the worker slot
until completion; a late result cannot insert text into a later attempt. Vox
retains the lease through its panel lifecycle, including shutdown.

Settings also offers a **Microphone** dropdown. Fresh or missing preferences use
**Built-in microphone**, resolved from CoreAudio's built-in transport metadata,
including the internal microphone on MacBooks. It does not guess from a device
name or select the first external input. Macs without an internal input show
Built-in microphone (unavailable); choose **System default** or a connected device.
Explicit devices are persisted by native UID, independent of Vox's WebKit IDs.
Devices refresh every two seconds while the main app is open. A missing saved
choice remains selected and marked unavailable; recording fails with a choice to
connect it or select another input, rather than silently switching microphones.

Changes apply to the **next recording**. The helper retains that preference after
main-app Quit and receives it again on reconnect. Each recording resolves its
choice once, selects it through `inputNode.auAudioUnit.setDeviceID`, verifies the
underlying input audio unit's CurrentDevice, and then reads its format before installing a tap. Selection must go through the
engine's AUAudioUnit owner: setting only the legacy AudioUnit property can leave
default-device tracking active and let a default aggregate replace the selected
microphone asynchronously during startup. Observation begins before engine start,
but the owner remains in a bounded startup phase until converted input arrives.
Startup health is distinct from active acceptance, so the helper publishes a
compact pending indicator until readiness. Startup buffers are discarded; a fresh
route, UID, format, permission and running-engine snapshot must then commit arming
before audio can be accepted. This uses
input readiness rather than a fixed sleep or a reset of a failed recording. No
input within two seconds of engine start returning fails the attempt. A setup
notification before this boundary cannot invalidate an accepted recording, because
none exists yet. A notification during arming or active capture remains a sticky
failure even if a later read looks healthy. Device, permission, unexpected route
and conversion failures also discard recording; stopping before readiness refuses
transcription.
System default is resolved at admission; changing the system input during capture
does not authorize switching the active recording to another microphone.

Insertion verifies Accessibility trust, frontmost PID, visible window identity,
secure-input state and original focus evidence immediately before every incremental Cmd-V. Each attempt and delivery has an identity;
only that attempt's own Cmd-V events carry its random CGEvent user-data tag and
helper source PID. Physical Cmd-V and unrelated synthetic events remain user input.
Known text controls can retain their AX object or a stable provider identifier.
Missing text semantics in custom/GPUI controls use a conservative fallback: the
original app/window and any exposed opaque AX root must remain the same. Keyboard
input other than the dictation shortcut, clicks, scrolling and app activation changes
invalidate that destination, even if it is later selected again. Supported AX
focus/window notifications trigger the same authoritative continuity check as
insertion: unchanged duplicates, including recreated text wrappers with the same
stable identifier, are accepted. An observed mismatch or unavailable read retires
the destination permanently; a later matching or unknown read cannot revive it.
No app is activated and no field is refocused.
Capture admission failure, missing permission, and a changed destination after
recording have separate recovery messages. It waits briefly for modifiers
to lift and otherwise discards. Every rich clipboard item/type is snapshotted
(up to 32 MiB, otherwise insertion is declined), each suffix is marked transient,
and restoration occurs after 800 ms only if the pasteboard change count still
matches. Further suffixes coalesce while that paste owns the clipboard; they never
replace an outstanding paste's contents. Restoration retains all rich formats and
leaves a newer user copy intact. Delivery offsets advance only once for each
attempted effect, preserving UTF-8 and whitespace; finalization never pastes the
whole transcript again. Deliberate helper shutdown waits through the same clipboard read window before
guarded restoration. There is
no acknowledgement from the destination app and no automatic replay after a paste.
Known secure fields and secure keyboard input are blocked. Vorkspace's secret
Input exposes a password role as well as masking its value. Native, browser and
custom controls can receive the same synthetic paste; a control still needs to
handle Cmd-V. An opaque app can change an internal responder programmatically
without exposing any AX notification or identity change; macOS offers no universal
field identity in that case. The fallback cannot certify that unobservable change.
Field validation does not prove the app accepted Cmd-V. Very slow apps and browser
pasteboard races can lose a paste; insertion is never retried automatically.

## Model and runtime

Inference uses static `transcribe.cpp` with Metal and embedded Metal kernels,
pinned to `7045d1c0fe7cee37747b2792977421d435e75fca`. The minimal local flow and
artifact catalog were inspected in Hex at
`1d4904c6bba9ab91f631c8c79108c3847b1b1c60`; retained MIT notices ship in Resources.
There is no cloud inference, rewriting, history or idle warm capture.

Unified uses `handy-computer/parakeet-unified-en-0.6b-gguf` revision
`7e948f21b7bdbac698d3318db9d350f1096f3b6c`, file
`parakeet-unified-en-0.6b-Q8_0.gguf`, exactly 731,357,568 bytes, SHA-256
`4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795`.
NVIDIA supplies the original model under its Open Model License; the conversion
catalog lists CC-BY-4.0 metadata. See bundled NOTICE and the linked model cards.

TDT uses [`handy-computer/parakeet-tdt-0.6b-v3-gguf`](https://huggingface.co/handy-computer/parakeet-tdt-0.6b-v3-gguf/tree/85ac09ea12fc4b1112fa76810059364bc6adc9de)
revision `85ac09ea12fc4b1112fa76810059364bc6adc9de`, file
`parakeet-tdt-0.6b-v3-Q8_0.gguf`, exactly 739,508,576 bytes, SHA-256
`5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7`.
The [NVIDIA original](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) and pinned
conversion list CC-BY-4.0. The runtime reads the GGUF head metadata and dispatches
to its TDT decoder (legacy v2/v3 default); Unified uses the same load/session API.
Unified supplies the English language hint; the pinned TDT artifact has no language
prompt input. TDT is a selectable alternative, without a Vorkspace runtime,
accuracy or performance qualification claim.

A typed model identity and descriptor in `vorkspace-core` own these pins; both
models share the helper's verification, download and inference implementation.
The preference persists the selected identity, including while dictation is off.
Helper control protocol 4 carries it in configuration and every model request
and notice, preserving the choice after Quit/reconnect and replacing older helpers
through the existing handshake. Capture and transcription snapshot the model.
A selection change cancels the old attempt before changing helper ownership;
its epoch and model identity fence insertion. Download identity and model identity
fence progress/readiness, retaining cancelled worker admission until completion.

Only the explicit model download uses the network. Download progress and errors
return over control IPC. Download writes an owner-only exclusive temporary file,
checks size and SHA-256, syncs, and renames last. A failed download leaves the
previous model intact. All user model files, download temporary assets and the
per-model lock live inside `~/.vorkspace/dictation/`. This path is independent of
bundle location, so installed and development helpers share the same artifact.
An exclusive kernel file lock grants download and orphan-cleanup authority to one
writer for that exact artifact across all helpers. The two published models,
locks and temporary downloads have distinct names and can coexist; cleaning up
one model never removes the other. Only that writer reaps abandoned downloads; a competing
live writer's temporary data is untouched. Published models remain readable while
another helper acquires a replacement. Initialization verifies the artifact off the UI thread; enabled worker
preparation verifies before loading. Turns reuse those immutable weights without
rehashing or loading them again. Missing HOME uses the operating system's account-home
resolver; unavailable home information produces an actionable error, never temporary
storage. Existing per-install caches are not automatically migrated.

Startup, configuration reads and disabled shortcuts never download a model.
Only the consent dialog's Download action admits network acquisition. A request
made during initial verification or helper reconnect waits rather than disappearing.
Settings Cancel also cancels an active acquisition. Its worker keeps admission
and the model lock until it returns and cleans its temporary file. Request IDs fence
late progress/completion; cancellation cannot enable a later attempt. Reconnect
recovers status or verifies the published artifact without replaying a download.
A competing install's writer reports a busy error; Retry checks the artifact again
and avoids fetching an already verified model.

## Isolated development

Build both binaries using `cargo build --locked --profile native-dev -p
vorkspace-gpui -p vorkspace-clipboard`, then package with
`bash scripts/desktop/bundle-native-macos.sh native-dev target 1.1.0 dev-data`.
This command does not install or launch. The helper bundle has its own microphone
usage description and audio-input signing entitlement. Static inference needs no
external runtime library or model bundled into the app.

Debug builds expose `vorkspace-clipboard --download-model-fixture /absolute/model.gguf`
and `--transcribe-fixture /absolute/model.gguf /absolute/mono-16khz-pcm16.wav` for
explicit, isolated Unified fixture checks (these debug-only commands use Unified). These do not start capture, register shortcuts,
or access the user's app database. Fixture transcription prints its result.

`vorkspace-clipboard --capture-health-fixture builtin` runs a debug-only,
2.5-second microphone health probe inside the helper's headless GPUI lifecycle.
It uses production device selection, conversion, health and stop, discards each
converted buffer immediately, and prints only non-content JSON metadata. It never
registers shortcuts, starts the supervisor, accesses a text target or clipboard,
loads a model, or transcribes. It needs Microphone permission for the signed helper
bundle; Accessibility is unnecessary. Without permission it reports a preflight
block. The explicit `--request-microphone` flag can show the macOS consent prompt;
only the user's Allow grants access. `--interrupt-fixture` injects an engine
configuration notification during the bounded probe to verify failure routing;
it must report a failed capture and interrupted stop. The diagnostic continues
observing a failure only until its fixed 2.5-second deadline, without retaining
audio or clearing the failure, so notification and first-buffer ordering can be
compared. Use an isolated signed
bundle and output directory; do not replace a running helper to diagnose capture.

`vorkspace-clipboard --live-dictation-fixture /absolute/model.gguf /absolute/public.wav`
feeds public 16 kHz mono PCM16 fixture audio at real-time cadence through the
production persistent worker twice, asserts committed text precedes Stop, and
prints fixture-only text and preparation/first-stable/stop-to-final timings. It
opens no microphone or destination and writes no user state. Debug-only
`--insertion-fixture PID success|edit|focus|cancel` preflights the signed helper's
Accessibility trust without prompting. It only admits a frontmost disposable
native field identified as `vorkspace-live-fixture`, then exercises production
incremental paste and refusal. `native/DictationFocusFixture.swift` supplies that
field with a known prefix. Missing trust is a live-proof blocker, not a passing
headless insertion test. Use a new isolated bundle; never replace a running helper.

Debug-only `--live-field-fixture /absolute/model.gguf /absolute/jfk.wav PID`
combines the production service's warm worker, bounded turn input, committed-event
handler, offsets and native paste owner. It accepts only the pinned public JFK
WAV and a frontmost disposable tagged field containing `Existing: `. It checks
actual AX field contents after each posted cumulative update, requires visible
stable text before Finish, checks final text once, and reports first-field and
Stop-to-final-field timings separately from the 800 ms clipboard read window.
Two turns reuse one load. Between completed turns only the disposable target
resets its own synthetic text, while still focused; the helper never writes or
refocuses through AX. Missing normal Accessibility trust refuses before loading
weights or attempting input. Run the fixed-string insertion scenarios first.
Use the same stable development signer when updating a permitted task bundle;
never replace a running bundle. This diagnostic starts no microphone or helper
service and accesses no user database or preferences.
