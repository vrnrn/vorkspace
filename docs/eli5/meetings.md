# Meeting notes

Choose **Meetings**, then **Record** at the bottom of the list, to transcribe your microphone and the sound from your meeting app. Press **Stop** when you finish. You can also use ⌘⇧R or choose **Stop Meeting Recording** from the vorkspace menu. Vorkspace saves the words, start time and duration. It does not save the audio.

Your Mac needs macOS 26 or newer, a downloaded speech language, and permission for microphone and system audio. Use headphones to help keep other people's voices out of your microphone track. The transcript labels the two audio sources without guessing who each person is. The recording keeps their timing aligned even when audio conversion delivers samples in uneven batches.

Meetings uses the same background helper as dictation. The Meetings page tracks recording progress and saved text. Dictation and Vox wait while a meeting uses the microphone. You can visit another page while recording. Quitting Vorkspace stops the meeting and keeps the transcript collected so far. If recording is interrupted, complete saved text remains available when you reopen the app.

In **Settings → Connectors → Meetings**, choose Codex, Claude, Pi, or Oh My Pi, a model, and instructions for your notes, then press **Save**. Clicking **Summarize** sends the written transcript to that provider. A cloud provider receives the text, and your audio stays on your Mac. Codex uses the account you already selected in Vorkspace and requires Codex CLI 0.166.0 or newer. If Vorkspace cannot verify the restrictions on Codex tools, it keeps the transcript on your Mac.

Use **Filter meetings** to find saved meetings. **Transcript** is the editable source, and **Summary** is the formatted preview. Choose **Delete** and confirm to remove a finished meeting and its text. You can change the title, correct the transcript, or edit the summary as Markdown. Press **Save changes** or ⌘S. A failed summary leaves your transcript and earlier notes intact. You can turn Meetings off in **Settings → Connectors → Plugins** after stopping any recording. Saved documents stay on your Mac. Draft edits stay in the current window when you turn Meetings back on.
