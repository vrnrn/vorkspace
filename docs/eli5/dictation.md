# Local dictation

Turn on Local dictation in Settings → General → Clipboard & Dictation. Download
the English speech model, then click Set up permissions. Allow Microphone and
Accessibility for **vorkspace Clipboard** in macOS Privacy & Security settings.
That is Vorkspace's small background helper, which also keeps clipboard history.

Click the field you want to write into. Press **⌘,**, speak, then press **⌘,**
again. A red microphone at the notch means recording. Amber with dots means the
Mac is turning speech into text. Leave the original field selected until the
words arrive. Switching fields discards the words. Pressing the shortcut during
transcription cancels it; Settings also has Cancel. Recording has a two-minute
limit. Vox uses **⌘.**. Settings is available from the application menu.

Everything is transcribed on your Mac. Downloading the model needs internet;
recording does not. Audio stays in memory temporarily and is never saved. There
is no dictation history. Long recordings are handled in short pieces, which may
affect wording and punctuation. Some password fields and custom controls cannot be used.

Dictation and clipboard history can be switched on separately. Either keeps the
helper alive when the main app quits. Switch both off, or use Quit in the helper's
menu, to stop it. Vox and dictation take turns using the microphone and notch.

The helper briefly puts the words on the clipboard and sends Paste once. It saves
all the previous clipboard formats and restores them only if you have not copied
something newer. Very large or unreadable clipboards are left alone and insertion
is declined. The helper cannot confirm whether another app accepted Paste, so it
never retries automatically. If something fails, check the message in Settings
and make a new recording. Your previous model survives a failed download; Retry
checks the new model before replacing it.
