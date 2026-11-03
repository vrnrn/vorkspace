# Dictation

Enable **Dictation** in **Settings → General**. It is off by default and uses
macOS 26's on-device `SpeechTranscriber` through `SpeechAnalyzer`. A compatible
Mac and supported system language are required; there is no cloud fallback.

Press **Fn / Globe** to start or finish in the focused text field.
Change that global shortcut under **Keybindings → App: Dictation**. Command comma
remains the Settings shortcut. Escape, typing, clicking, scrolling or changing
focus cancels further insertion. Text already delivered stays in the field.
The notch's microphone button finishes; its cross cancels.

The microphone turns red and its input meter appears after prepared recognition
and native audio input are ready. Starting and finishing use a white indicator.
The surface never activates the main window. It shares Vox's notch geometry and
uses a top-center fallback on displays without a notch. An exclusive lease keeps
Vox and dictation from opening competing microphone and notch owners.

## Setup and custom words

The bundled **vorkspace helper** app needs Microphone and Accessibility
permission. Settings names the missing access and offers its repair action.
If Apple's language files are missing, **Download language** explicitly requests
installation. macOS owns, shares and updates those files. Startup and disabled
dictation never initiate a download.

Enter one preferred spelling per line and choose **Save custom words**:

```text
Vorkspace
work space → Vorkspace
clawed → Claude
```

A term on its own fixes capitalization. An alias corrects a specific misheard
phrase; `=>` is also accepted. Matches ignore case, normalize Unicode and
whitespace, respect whole-word boundaries and preserve surrounding punctuation.
The longest phrase wins; replacements do not cascade. Conflicting spellings are
refused before saving. Limits are 100 entries, 120 characters per side and 16 KiB
in total. Microphone and word changes apply to the next turn. Unrelated preference
updates preserve an unsaved word draft.

These are local text corrections applied to Apple's live output. They do not
train or bias `SpeechTranscriber` acoustically. Apple's documented vocabulary
hints apply to `DictationTranscriber`, a different module; this feature keeps the
newer engine. Punctuation comes from Apple without a separate rewriting step.

## Ownership and delivery

The persistent background helper owns the shortcut, native session and notch.
Dictation keeps it alive independently of clipboard history, including after the
main app quits. Disabling both features or choosing Quit in the clipboard menu
stops the helper. Reconnection sends configuration and status; recognized text
and audio never cross the control socket or get replayed.

Swift owns each turn: destination admission, locale reservation, analyzer
preparation, pinned microphone, result stream, finish, cancellation and cleanup.
The microphone is closed while idle. Capture converts owned PCM buffers into a
bounded analyzer stream; overflow fails the turn instead of dropping audio.
Audio is transient and is never saved or logged. Recording finishes after two
minutes. Cancellation stops capture immediately and fences publication; the
helper retains its lease through actual cleanup.

Apple's finalized phrases form an immutable prefix. Its current provisional
phrase is replaced as results arrive, with custom words applied to the combined
text. For accessible editable fields, insertion changes only its own range and
checks the caret and previously inserted span. Other responders receive tagged
Unicode keyboard events and tail erases, without using the clipboard. Real input,
activation, focus changes and secure input permanently revoke the destination.
Modifier keys briefly defer publication. Uncertain external effects are never
retried. Custom responders may handle synthetic input differently, so actual app
compatibility needs runtime verification.

`crates/vorkspace-helper/native/` owns speech and insertion. The Rust helper
owns the shortcut, lease and presentation; `vorkspace-macos` owns shared notch
placement. Native regressions run under `npm run test:tooling`; helper lifetime
and reconnection tests run under `npm run test:rust`.

## Dictation key and history

The default is `fn`; `right-alt` is the right Option key. Settings → General →
Dictation chooses either key, and Keybindings can supply a modifier chord. One
binding handles both gestures: a release before 250 ms toggles; holding starts
(or continues) a turn and release finishes it. Chording Fn or Option with another
key cancels pending activation. A session event tap consumes the dictation key;
macOS Keyboard’s Globe-key action should be Do Nothing to avoid system conflicts.

The speech owner publishes its last confirmed delivered text once per turn, after
cleanup. The helper adds it as one history entry when history is enabled. Text
revisions never touch the pasteboard, so unrelated user copies are preserved.
Clearing history increments its epoch; a turn already in progress cannot add its
text afterward. A new turn uses the new epoch.
The helper retains its bundle identifier and legacy per-install socket identity
through the rename so updates can replace the previous helper.

Meetings reuses this helper and speech engine for microphone plus system-audio transcription. Stop a meeting before starting dictation. Meeting recording finishes on application Quit; enabled dictation remains available afterward. See [Meetings](meetings.md).
