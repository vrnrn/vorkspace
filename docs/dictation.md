# Local dictation

Dictation is **off by default**, including old preferences that have no dictation
setting. In Settings → General → Dictation, the **Model** dropdown
chooses **Parakeet Unified English** (the default), **Parakeet-TDT v3**, or
**Parakeet Flash**. TDT v3 and Flash are experimental. Flash is offered on Apple silicon with macOS 14 or newer.
Unified and TDT are 0.6B Q8 GGUF artifacts; Flash is a 120M Core ML model.
Selecting a model stops dictation and leaves it off;
selection alone never enables capture. If the selected artifact is missing,
Settings asks for Download or Cancel, naming that model, its approximate size
(**731 MB** Unified, **740 MB** TDT, **224 MB** Flash), and
`~/.vorkspace/dictation/` as destination.
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

While enabled, the helper verifies and loads the selected weights once, prewarms
the selected model with synthetic silence, and publishes **Preparing** until
that work finishes. It keeps one compatible model warm without opening the mic.
Each turn gets a new session; its scratch arenas are freed when the turn joins.
Disabling, changing models or publishing a download fences old work and joins the
worker before starting a successor. Disabled dictation never loads weights.

Press **⌘,** once to start. An amber microphone means preparation or microphone
startup; wait for the red microphone before speaking. Red and **Recording** appear
only after the prepared session and native capture owner can accept audio.
The models insert stable words directly into the focused field during capture.
Tentative hypotheses never leave the helper. With no custom words, Unified's native windows advance every 560 ms
with 560 ms right context and the model's committed token boundary; a trailing word waits for
a whitespace boundary, one second of quiet without a text change, or finalization.
Quiet is measured against both an absolute floor and recent speech, so a steady
low background level does not indefinitely hold the last stable word.
Press **⌘,** again to drain
and finalize; only the remaining suffix of a final hypothesis that extends the
source prefix already accepted by the correction pipeline is inserted. Trailing
whitespace waits until its position is known, so punctuation can precede it
without moving the cursor. For Unified English only, an explicit Stop can add
one `?` after correction if the model leaves a short, plain-English direct
question unpunctuated. A narrow text heuristic considers the final sentence and
some conversational openings after a comma. It refuses ambiguous boundaries,
many question forms, recognizable numbers, and obvious technical text. It does
not run on live partials, automatic capture limits, TDT or Flash. It does not analyze
prosody or determine intent: it can miss a question or mistake a fragment or
statement for one. Existing punctuation wins. Missing periods are not guessed.
The question mark is an ordinary append-only suffix subject to the same
cancellation and destination checks as recognized text; aliases are resolved
before it is considered.
Unified's separate offline decode is not an interchangeable final result: it
can revise words and punctuate a fragment or a question as a statement. It is
therefore not used to replace or extend the live transcript.
The nonactivating notch shows only the right-hand microphone indicator. On a
notched screen its black body overlaps the obscured notch area; without a notch,
the indicator is centered at the top of the screen. Status and errors remain
available to accessibility and in Settings.
TDT v3 uses repeated inference on overlapping audio windows with the same warm
offline model. Preview windows are processed about every 600 ms; the helper waits
for three agreeing word hypotheses and additional right context before insertion.
After quiet, it can release an agreed word while holding unstable trailing
punctuation until the next word or final result. Numeric words keep the stricter
hold because their written form can change between previews.
Previously inserted words are matched by text and time across window changes. If
an overlap cannot be reconciled safely, insertion stops rather than replaying it.
Flash uses the FluidAudio EOU engine's cached streaming encoder and decoder,
keeping utterance-boundary hints from ending the recording. Its state resets
between dictation turns, not for each raw boundary hint.
Its English output has no punctuation or capitalization. A trailing word
uses the same quiet-period hold as Unified.

### Custom words

In **Custom words**, enter one preferred term per line, for example `Vorkspace`,
and choose **Save custom words**. If a term is misrecognized, add a line such as
`work space => Vorkspace`.
The left side is what the recognizer may output; the right side is the desired
text. A rule can also correct capitalization by using the same words on both
sides. Rules are local, case-insensitive and Unicode-normalized; whitespace
within a phrase can vary, punctuation in a rule must match literally, and
surrounding punctuation is kept. Matches respect word boundaries; when rules
overlap, the longest match wins. A recording snapshots the saved rules when it
starts, including after the main app quits; changes apply to the next recording.

All models apply these exact text corrections locally before insertion. In
addition, TDT supplies preferred spellings and heard aliases to
transcribe.cpp's Parakeet acoustic vocabulary rescoring. The decoder limits the
number and tokenization of eligible words and may skip a term it cannot encode;
acoustic scoring is not a guarantee that a word will be recognized. Unified's
decoder rescores only at finalization and could revise already inserted text, so
Unified and Flash use only text corrections, including case correction for a
standalone term. Text corrections are deterministic and are not acoustic bias.
The helper holds incomplete possible aliases across chunks and segments and never revises
an inserted prefix; cancellation discards the still-held suffix. A trailing
word or phrase that could match a correction may wait for a word boundary or
Stop so the rule can be applied without rewriting text already inserted.

Starting-toggle cancels without transcription. Another toggle while finishing,
Settings Cancel, real typing, clicks/navigation, focus/app changes, input loss or
model change stops further insertion. Words already inserted remain untouched.
A cancelled stream is never replayed, even after an uncertain paste or stream
error. Recording stops after two minutes. No microphone opens while idle. Audio
is transient, never written or logged. The existing 16 kHz native capture owner
drains incremental PCM through an eight-second native buffer and an eight-second
worker queue (up to 80 blocks of at most 100 ms). Overflow fails the turn
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
Updated helpers for the same macOS account also acquire a separate model lease
before loading any dictation engine. It uses the account home resolved by the
operating system, independent of bundle location, HOME and TMPDIR; it does not
control clipboard history or Vox. The helper retains the lease through actual
worker join, including cancellation and load failure. A waiting helper reports
the conflict and retries automatically. A crash releases the kernel lock. The
owner-only `~/.vorkspace-dictation-engine.lock` file is retained even when idle:
removing a live lock file would defeat coordination. Older helpers which do not
use this lease can still load another engine; this is not a guarantee against
non-cooperating processes. The separate shared voice lease admits only one
microphone/notch owner among processes using the same per-user temporary
directory; a different temporary-directory environment can create a separate
voice lease.

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
recording have separate recovery messages. It waits briefly for the actual
Command, Control, Option and Shift keys to lift and otherwise discards. Paste
events enter at the session tap so their synthetic modifier flags do not alter
hardware key state. Every rich clipboard item/type is snapshotted
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

Unified and TDT inference use static `transcribe.cpp` with Metal and embedded Metal kernels,
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

Flash uses the 160 ms Core ML snapshot from
[`FluidInference/parakeet-realtime-eou-120m-coreml`](https://huggingface.co/FluidInference/parakeet-realtime-eou-120m-coreml/tree/40a23f4c0b333aa17ad8c0f2ea47ec2347f2f355/160ms),
revision `40a23f4c0b333aa17ad8c0f2ea47ec2347f2f355`, and the adapted
FluidAudio EOU engine pinned at `386f7195670b77f2954c7ee143ec63fcb50b6c3b`.
The snapshot contains three compiled Core ML models and a vocabulary. NVIDIA's
original is governed by the NVIDIA Open Model License. Every file is checked
with SHA-256; Git-tracked metadata also uses its pinned Git blob identity. Flash loads
only verified local files, uses the Neural Engine and CPU, and never downloads
from the inference engine.

A typed model identity and descriptor in `vorkspace-core` own these pins; all
models share the helper's lifecycle and consent flow.
The preference persists the selected identity, including while dictation is off.
Helper control protocol 7 carries it and the saved custom-word configuration, and carries model identity in every model request
and notice, preserving the choice after Quit/reconnect and replacing older helpers
through the existing handshake. Capture and transcription snapshot the model.
A selection change cancels the old attempt before changing helper ownership;
its epoch and model identity fence insertion. Download identity and model identity
fence progress/readiness, retaining cancelled worker admission until completion.

Only the explicit model download uses the network. Download progress and errors
return over control IPC. GGUF downloads use an owner-only exclusive temporary file;
Flash uses an owner-only staging directory with each pinned member verified.
Downloads sync and publish last. A failed download leaves the
previous model intact. All user model files, download temporary assets and the
per-model lock live inside `~/.vorkspace/dictation/`. This path is independent of
bundle location, so installed and development helpers share the same artifact.
An exclusive kernel file lock grants download and orphan-cleanup authority to one
writer for that exact artifact across all helpers. The three published models,
locks and temporary downloads have distinct names and can coexist; cleaning up
one model never removes another. Only that writer reaps abandoned GGUF downloads; a competing
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

Debug builds expose `vorkspace-clipboard --download-model-fixture /absolute/model [unified|tdt-v3|eou]`
for explicit, isolated downloads and `--transcribe-fixture /absolute/model.gguf
/absolute/mono-16khz-pcm16.wav` for a Unified check. These do not start capture,
register shortcuts, or access the user's app database. Fixture transcription
prints its result. Downloads still require explicit consent.

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

`vorkspace-clipboard --live-dictation-fixture /absolute/model /absolute/public.wav [unified|tdt-v3|eou] [repeats] [turns] [gap-seconds] [rules]`
feeds public 16 kHz mono PCM16 fixture audio at real-time cadence through the
production persistent worker (twice by default), asserts committed text precedes Stop, and
prints fixture-only text and preparation/first-stable/stop-to-final timings. It
opens no microphone or destination. It acquires the normal model lease; for an
isolated fixture, `VORKSPACE_DICTATION_FIXTURE_LOCK` can name an absolute test
lock path. Debug fixture options can select a second absolute WAV with
`VORKSPACE_DICTATION_FIXTURE_ALTERNATE_WAV` or path-separated per-turn WAVs with
`VORKSPACE_DICTATION_FIXTURE_WAVS`, feed comma-separated per-turn milliseconds of
silence after Stop with `VORKSPACE_DICTATION_FIXTURE_TAIL_MS`, or print the Unified stream's raw,
committed and held public-fixture text with `VORKSPACE_DICTATION_FIXTURE_TRACE`.
These options are used only by the explicit fixture, never ordinary capture.
Debug-only `--dictation-delivery-fixture /absolute/lock /absolute/model /absolute/question.wav /absolute/statement.wav [unified|tdt-v3|eou]`
feeds two clips through the production Service and worker into a memory-only
paste substitute. It checks live delivery, ordered suffixes and a simulated
clipboard read window without accessing a real field or clipboard. The scripts
in `crates/vorkspace-clipboard/tests/` check continuous
public speech, noisy pauses on all engines and real multi-process model-owner
transitions using explicitly supplied existing models and isolated paths. The debug-only
`--unified-final-compare /absolute/lock /absolute/model /absolute/clip.wav [...]`
compares the production streaming worker and offline decoding sequentially on
the same warm Unified model. It feeds the chunks synchronously without real-time
cadence, prints text from the explicitly supplied public or generated WAV files,
and never opens a microphone or destination. Its model comparison uses an
automatic-finish reason, so its streaming result excludes the Stop heuristic.
`--dictation-delivery-matrix /absolute/lock /absolute/model /absolute/cases.json`
feeds Unified fixtures through the production Service and worker with one warm
model and a memory-only destination. The JSON array contains `wav` (absolute
PCM16 mono 16 kHz path), `expected` (exact final worker text), and `action`
(`deliver`, `cancel`, `edit`, or `focus`), plus optional `expected_live` (exact
text delivered before Stop), `automatic` (finish without a user Stop), and
`expect_no_suffix` (successful completion without another insertion). It
checks live delivery, the pending suffix while the simulated clipboard read
window is busy, and stale delivery after invalidation. Each clip must produce
live text; it normally also needs a final suffix. These
memory checks do not establish what a real field did.
The debug-only
`--tdt-window-fixture /absolute/model.gguf /absolute/public.wav` also prints TDT
window hypotheses and word timings for the supplied fixture. Debug-only
`--insertion-fixture PID success|edit|focus|cancel` preflights the signed helper's
Accessibility trust without prompting. It only admits a frontmost disposable
native field identified as `vorkspace-live-fixture`, then exercises production
incremental paste and refusal. `native/DictationFocusFixture.swift` supplies that
field with a known prefix. Missing trust is a live-proof blocker, not a passing
headless insertion test. Use a new isolated bundle; never replace a running helper.

Debug-only `--live-field-fixture /absolute/model /absolute/public.wav PID [unified|tdt-v3|eou]`
combines the production service's warm worker, bounded turn input, committed-event
handler, offsets and native paste owner. It accepts only the pinned public JFK
WAV for Unified or the pinned English FLEURS WAV for TDT and Flash, and a frontmost
disposable tagged field containing `Existing: `. It checks
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
