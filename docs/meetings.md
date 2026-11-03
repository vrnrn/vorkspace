# Meetings

Open **Meetings** in the left sidebar. **Record** (⌘⇧R) starts on-device transcription of the selected dictation microphone and system audio. **Stop** finishes both sources and saves a document. The list, search, editable transcript and Markdown summary stay in one Notes-style view; a narrow pane switches between list and document.

Meetings requires macOS 26+, a supported installed Apple speech language, Microphone permission and Screen & System Audio Recording permission for **vorkspace helper**. Install the language from Dictation settings first. Capture attaches only system-audio output, never screen frames. Audio is processed in memory and is not retained as a recording. Headphones help avoid the microphone also hearing remote participants through speakers. “Microphone” and “System audio” identify sources, not individual speakers.

## Recording lifetime

In the desktop app, `background_helper.rs` owns the helper connection and process lifetime. `meetings/capture.rs` consumes its recording events and owns the app-side recording state. The native helper owns capture admission, one shared microphone lease, a language reservation, and both speech analyzers. Dictation and Vox cannot start while Meetings owns the lease. Recording becomes ready after both sources are ready. Stop remains available while preparing and through **vorkspace → Stop Meeting Recording**; finalization keeps the lease until both native captures and analyzers have actually closed. Leaving the page keeps recording; application Quit or a lost desktop connection finishes it. The helper can remain running for clipboard history or dictation afterward.

Each finalized phrase is appended with a sequence, source and recording-relative start/end time, then flushed before publication. Microphone host time and system-audio timestamps share one clock origin. Real clock gaps are preserved; backwards timestamps fail rather than corrupting ordering. Live UI updates are bounded, and a sequence gap reloads durable text. No audio file is produced.

Meeting data lives below the desktop data directory at `meetings/`. Existing records in the earlier `plugins/meetings/data/` location are reopened in place. The layout inside either directory is:

- `recordings/<id>/state.json`, `owner.lock` and `transcript.jsonl` hold recorder metadata and immutable source text.
- `meetings/<id>.json` holds the title, editable transcript override, revision and optional summary.
- `settings.json` holds provider, model and summary prompt.
- `analysis/<id>.lock` prevents duplicate analysis of one meeting. The saved document is the authority for the summary.

Metadata is atomically replaced after flushing; an exclusive owner lock distinguishes a live recorder from abandoned work. Reopening an abandoned recording marks it interrupted, leaves the end time unknown, and retains complete saved phrases. An incomplete final line cannot replace earlier transcript text. Storage errors stop capture; they never advertise a successful save. A missing publication marker is not interpreted as an empty finished recording.

## Summaries

Open **Settings → Connectors → Meetings**, select Claude, Pi, or Oh My Pi, choose or enter a model, edit the summary prompt, and press **Save**. Settings use the provider account already selected in Vorkspace. Audio stays on the Mac; clicking **Summarize** sends transcript text to the chosen provider, which may be a cloud service.

Each generation pass has a three-minute deadline within a 15-minute budget for the whole summary. Intermediate passes preserve details relevant to the requested notes.

The default prompt produces Summary, Discussion, Decisions, Action items and Open questions. It asks for factual notes, useful timestamps and explicit uncertainty, with owners or due dates only when stated. Long transcripts are processed in bounded sections and reduced before applying the final format. Analysis uses the selected provider’s tool-disabled mode from a private temporary working directory. Provider accounts are still selected from the active project’s settings.

**Transcript** keeps the source available. **Summary** displays polished Markdown, and **Edit Markdown** edits that summary. **Save changes** (⌘S) saves title and document edits. Updates validate the saved revision. A late summary cannot overwrite a meeting that changed while analysis ran; provider failures preserve the original transcript and any previous summary. Editing the transcript marks an older summary as stale until it is regenerated.

Rendered summaries strip raw HTML, images and unsafe link destinations. The bundled Meetings page uses a dedicated message handler for recording and document actions. Other HTML surfaces have no access to it. **Settings → Connectors → Plugins** can hide Meetings while keeping saved documents. Finish an active recording before turning it off. Unsaved edits stay in the current window when Meetings is turned off and back on.
