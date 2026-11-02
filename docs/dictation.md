# Local dictation

Dictation is **off by default**, including old preferences that have no dictation
setting. Choose **Local dictation** in Settings → General → Clipboard & Dictation.
Settings checks the shared model first. If it is already verified, dictation is
enabled without another download. Otherwise a consent dialog names **Parakeet
Unified English**, its approximate **731 MB** size, and the destination
`~/.vorkspace/dictation/`, with **Download** and **Cancel** actions. The saved
preference remains off until download and integrity verification succeed. Cancel
or dismiss the dialog to leave it off. Use **Set up permissions** afterward. macOS must allow
Microphone and Accessibility for **vorkspace Clipboard**, the nested helper at
`vorkspace.app/Contents/Helpers/vorkspace Clipboard.app`. Denied permissions can
be changed in System Settings → Privacy & Security. Development bundles have
their own executable location; authorize that helper when testing it.

Press **⌘,** once to record and again to finish. The nonactivating notch has an
empty left wing and a red microphone on the right while recording. An amber
microphone and ellipsis indicate local transcription. Keep the intended text
field selected until insertion. Switching fields discards the result. Another
toggle during transcription cancels it. Settings also has Cancel. Recording stops
automatically after two minutes. No microphone is open while idle; audio is held
only in bounded memory and never written or logged. Long clips are inferred in
20-second chunks; wording and punctuation across those boundaries may differ.

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

Insertion verifies Accessibility trust, frontmost PID and the original focused
AX element immediately before sending one Cmd-V. It waits briefly for modifiers
to lift and otherwise discards. Every rich clipboard item/type is snapshotted
(up to 32 MiB, otherwise insertion is declined), the transcript is marked transient,
and restoration occurs after 800 ms only if the pasteboard change count still
matches. Deliberate helper shutdown waits through the same clipboard read window before
guarded restoration. There is
no acknowledgement from the destination app and no automatic replay after a paste.
Some secure or custom controls do not expose an accessible focused element; those
controls are unsupported. Field validation does not prove the app accepted Cmd-V.

## Model and runtime

Inference uses static `transcribe.cpp` with Metal and embedded Metal kernels,
pinned to `7045d1c0fe7cee37747b2792977421d435e75fca`. The minimal local flow and
artifact catalog were inspected in Hex at
`1d4904c6bba9ab91f631c8c79108c3847b1b1c60`; retained MIT notices ship in Resources.
There is no cloud inference, rewriting, history or idle warm capture.

The model is `handy-computer/parakeet-unified-en-0.6b-gguf` revision
`7e948f21b7bdbac698d3318db9d350f1096f3b6c`, file
`parakeet-unified-en-0.6b-Q8_0.gguf`, exactly 731,357,568 bytes, SHA-256
`4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795`.
NVIDIA supplies the original model under its Open Model License; the conversion
catalog lists CC-BY-4.0 metadata. See bundled NOTICE and the linked model cards.
Only the explicit model download uses the network. Download progress and errors
return over control IPC. Download writes an owner-only exclusive temporary file,
checks size and SHA-256, syncs, and renames last. A failed download leaves the
previous model intact. All user model files, download temporary assets and the
per-model lock live inside `~/.vorkspace/dictation/`. This path is independent of
bundle location, so installed and development helpers share the same artifact.
An exclusive kernel file lock grants download and orphan-cleanup authority to one
writer across all helpers. Only that writer reaps abandoned downloads; a competing
live writer's temporary data is untouched. Published models remain readable while
another helper acquires a replacement. Initialization and each inference validate
the model off the UI thread. Missing HOME uses the operating system's account-home
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
explicit, isolated fixture checks. These do not start capture, register shortcuts,
or access the user's app database. Fixture transcription prints its result.
