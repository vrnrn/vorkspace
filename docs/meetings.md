# Recording in Notes

Open **Notes** in the left sidebar and choose an existing note or press **+** above the list. Every note offers **Start recording** and an audio source picker. Opening a blank note does not start capture or save an empty note. An unnamed empty capture on an otherwise blank note stays out of saved Notes. **Stop recording** finishes capture; starting again appends to the same attached recording and preserves transcript corrections. A narrow window switches between the Notes list and document; Back retains unfinished text and reading position.

**Preview** and **Source** display and edit the note's authored Markdown. Once a recording is attached, **Summary** and **Transcript** appear beside them. These flat tabs underline the active section. The recording's generated summary and transcript remain separate from the authored note body. In **Summary**, **Edit summary** opens its Markdown editor and **View summary** returns to the formatted result.

Notes discovers recordings without a linked note through a catalog of recording IDs, titles and start times. Import preserves the recording's original identity and any pending title. Discovery skips recordings that already have a linked note. Opening a recording loads its full document.

**Copy** and **Add to chat** use the selected section's text without replacing the authored body. Sharing a live transcript waits for capture to finish so provisional speech is not mistaken for the final transcript.

Choose **Mic + system audio** for calls or **Mic only** for an in-person meeting. This choice belongs to the recording and can change between takes. Recording requires macOS 26+, a supported installed Apple speech language and Microphone permission for **vorkspace helper**. System audio also requires Screen & System Audio Recording permission; Mic only neither requests that permission nor creates a system capture. Install the language from Dictation settings first. Capture attaches only system-audio output, never screen frames. Audio is processed in memory and is not retained as a recording. Headphones help avoid the microphone also hearing remote participants through speakers. “Microphone” and “System audio” identify sources, not individual speakers.

## Recording lifetime

In the desktop app, `background_helper.rs` owns the helper connection and process lifetime. `meetings/capture.rs` consumes its recording events and owns the app-side recording state. The native helper owns capture admission, one shared microphone lease, a language reservation, and the requested speech analyzers. Dictation and Vox cannot start while a recording owns the lease. Recording becomes ready after all requested sources are ready. Stop remains available in Notes while preparing; finalization keeps the lease until native captures and analyzers have actually closed. Leaving the page keeps recording; application Quit or a lost desktop connection finishes it. The helper can remain running for clipboard history or dictation afterward.

The clipboard menu has one meeting action above **Settings…**: **New meeting** opens a blank note ready to record, and **Stop recording** replaces it while capture is active. Quick Composer remains available through its global shortcut. The helper also owns a nonactivating notch indicator using the shared macOS notch geometry. Its left wing shows a red dot and recorded duration; its right wing opens the active note or stops capture. It remains visible when the workspace is hidden and closes on the terminal recording receipt.

Each finalized phrase is appended with a sequence, source and recording-relative start/end time, then flushed before publication. Microphone host time and system-audio timestamps share one clock origin. The raw timestamps detect gaps and backwards input. Within a continuous run, the analyzer clock advances by the number of frames the converter actually emits. Live UI updates are bounded, and a sequence gap reloads durable text. No audio file is produced.

Each Start has a unique run identity and compares the previous durable run under the recorder lock. Receipts, partial speech, Stop controls and admission timeouts carry that identity, so a delayed event from an earlier take cannot affect a later one. The transcript log continues its sequence and offsets timestamps by previously captured duration. An edited transcript records the sequence it replaced; later phrases append after that prefix. New transcript content makes an earlier summary stale.

SQLite owns note titles, authored Markdown, tags and project associations. Its `note_recordings` relation links each note to the recording's existing ID, including an identity reservation before capture starts. A reserved recording link does not by itself create a saved note. Existing visible meeting records are imported into Notes once; subsequent loads preserve note edits. Deletion tombstones prevent later imports or delayed saves from recreating a deleted note.

Recording files remain below the desktop data directory at `meetings/`. Existing records in the earlier `plugins/meetings/data/` location are reopened in place, without moving transcript files. Both locations use the same layout.

- `recordings/<id>/state.json`, `owner.lock` and `transcript.jsonl` hold recorder metadata and immutable source text.
- `meetings/<id>.json` holds the editable transcript override, recording document's revision and optional summary. Its legacy title does not override the note's SQLite title.
- `drafts/<id>.json` holds the recording attachment's edit checkpoint and the saved revision it was based on.
- `drafts/<id>.lock` lets one window edit the recording attachment at a time.
- `settings.json` holds provider, model, optional effort and summary prompt.
- `analysis/<id>.lock` prevents duplicate analysis of one recording. The recording document is the authority for the transcript and summary.

Metadata is atomically replaced after flushing; an exclusive owner lock distinguishes a live recorder from abandoned work. Reopening an abandoned recording marks it interrupted, leaves the end time unknown, and retains complete saved phrases. An incomplete final line cannot replace earlier transcript text. Storage errors stop capture; they never advertise a successful save. A missing publication marker is not interpreted as an empty finished recording.

## Summaries

Open **Settings → Connectors → Notes**, select Codex, Claude, Pi, or Oh My Pi, choose a model and effort, edit the summary prompt, and press **Save**. The model picker displays the selected model; **Custom model…** reveals an ID field when needed. Effort choices come from the selected account’s model catalog. Changing model or provider clears the previous effort. **Default** leaves effort to the provider, and older settings without an effort value remain readable. Settings use the provider account already selected for the active project. Audio stays on the Mac. After stopping capture, clicking **Summarize** sends transcript text to the chosen provider, which may be a cloud service.

Each generation pass has a three-minute deadline within a 15-minute budget for the whole summary. Intermediate passes preserve details relevant to the requested notes.

The default prompt produces Summary, Discussion, Decisions, Action items and Open questions. It asks for factual notes, useful timestamps and explicit uncertainty, with owners or due dates only when stated. Long transcripts are processed in bounded sections and reduced before applying the final format. Mandatory output instructions apply to custom prompts and intermediate passes: return only finished meeting-note content, without model reasoning, work steps or progress commentary. Existing saved summaries and custom prompt settings are not rewritten.

The provider pipeline selects one completed final answer and publishes it only after successful completion. Codex uses the completed `final_answer` message; for legacy models that omit phase labels, it uses the last completed unphased assistant message only when later phase evidence does not contradict it. Explicit commentary and partial answers are excluded. Claude uses its successful final result; Pi and Oh My Pi use the last completed assistant message's text blocks. Empty or unfinished finals, failed turns and interrupted requests cannot replace the saved summary. An explicitly empty final never falls back to earlier text.

Generation runs in a private temporary working directory. Codex requires CLI 0.166.0 or newer. Vorkspace starts an ephemeral Codex thread without an execution environment and verifies restrictions on shell, web, browser, agents, hooks, and MCP access before sending the transcript. If setup fails, the transcript stays local.

## Editing and deletion

Notes saves authored title, body and tag changes through its SQLite save path. **Transcript** edits the attached recording's text; **Edit summary** edits its generated Markdown. **Save edits** or ⌘S commits attachment edits. Updates validate the saved revision. A late summary cannot overwrite a recording document that changed while analysis ran; provider failures preserve the transcript and any previous summary. Editing the transcript marks an older summary as stale until it is regenerated.

The attachment editor coalesces draft checkpoints and flushes pending input on normal close and Quit. Reopen the note to restore attachment drafts and their edit position. If the saved revision changed elsewhere, the editor preserves the draft text and offers **Discard edits** to reload the saved version. An attachment open for editing in one window is read-only in other windows. After that editor releases it, reopen the note to acquire editing.

**Delete** and confirmation remove the note and its finished recording attachment. The recording root is retired under the recorder lock before cleanup, and the durable link between note and recording keeps a deletion tombstone so delayed work cannot republish it.

The saved summary Markdown is the source for the shared native Markdown renderer. Summary previews omit raw HTML and images and allow only HTTP, HTTPS and email links. The **Enable notes** toggle in **Settings → Connectors → Notes** controls Notes and its recording controls while preserving saved data. Finish an active recording before turning Notes off.
