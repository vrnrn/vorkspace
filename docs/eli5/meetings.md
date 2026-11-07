# Record a meeting in a note

Choose **Notes**, then open a note or press **+** above the list. Press **Start recording** when you are ready. You can record in any note. Stop and start as often as needed: each take continues the same recording, including any transcript corrections. Opening an untouched blank note does not save it. A blank, unnamed recording with no transcript does not add an empty note to your saved list. Vorkspace saves the words, start time and duration. It does not save the audio.

**Preview** shows the text you wrote in the note; **Source** lets you edit that text. A recording adds **Summary** and **Transcript** tabs beside them. The active tab has a line underneath it. Your written note, the transcript and the generated summary stay separate. To change a summary, open **Summary** and choose **Edit summary**, then **View summary** to return to its formatted view.

**Copy** and **Add to chat** use the section you are viewing. This does not overwrite your written note. Finish recording before sharing its transcript.

Choose **Mic + system audio** for an online call or **Mic only** for an in-person meeting. Your Mac needs macOS 26 or newer, a downloaded speech language, and microphone permission. Mic only does not ask for system-audio permission. Use headphones on calls to help keep other people's voices out of your microphone track. The transcript labels audio sources without guessing who each person is. The recording keeps their timing aligned even when audio conversion delivers samples in uneven batches.

The menu bar has **New meeting** above **Settings…**. It opens a blank note ready to record. During recording it becomes **Stop recording**. A red dot and timer also appear around the notch, with buttons to open the note and stop recording. This indicator stays visible when you hide the main window. Quick Composer remains available through its global shortcut.

Recording uses the same background helper as dictation. Notes shows recording progress and saved text. Dictation and Vox cannot start while a recording uses the microphone. You can visit another page while recording. Quitting Vorkspace stops the recording and keeps the transcript collected so far. If recording is interrupted, complete saved text remains available when you reopen the app.

In **Settings → Connectors → Notes**, choose Codex, Claude, Pi, or Oh My Pi, a model, effort, and instructions for your summary, then press **Save**. Choose **Custom model…** if you need to enter a model ID. The effort menu offers the levels supported by that model; **Default** uses the provider’s choice. Picking another model or provider resets effort to Default. After recording stops, clicking **Summarize** sends the written transcript to that provider. A cloud provider receives the text, and your audio stays on your Mac. The provider uses the account already selected for your project. Codex requires Codex CLI 0.166.0 or newer. If Vorkspace cannot verify the restrictions on Codex tools, it keeps the transcript on your Mac.

Summaries contain the finished meeting notes. Instructions always ask the model to leave out its reasoning, work steps and progress commentary, including when you customize the prompt. Vorkspace saves the completed final answer only after the provider finishes successfully. Older Codex models without message labels use their last completed message. An empty answer, unfinished response or failed request leaves the transcript and earlier summary intact. Existing summaries are left as you saved them.

Use **Filter notes** to find your notes. You can change the note's title, written text and tags as usual, or correct its recording in **Transcript**. **Save edits** or ⌘S saves transcript and summary changes. Summary formatting skips embedded HTML and images, and links open only web and email addresses. Choose **Delete** and confirm to remove a note and its finished recording.

Vorkspace keeps unfinished transcript and summary edits in a draft on your Mac. Reopen the note to pick up those edits where you left off. If the saved recording changed elsewhere, you can copy your draft text and use **Discard edits** to reload the saved version. Only one window can edit a recording attachment at a time. In another window, you can read it and reopen the note for editing after the first window leaves it. In a narrow window, going Back to the list keeps your unfinished text and reading position.

Earlier saved meetings appear in Notes with their transcripts and summaries still attached. Their transcript files stay where they were. Once imported, later loads keep your title, text and tag changes. Deleted recordings do not reappear when older files are found again.

You can turn Notes and its recording controls off in **Settings → Connectors → Notes** after stopping any recording. Saved notes and recordings stay on your Mac.
