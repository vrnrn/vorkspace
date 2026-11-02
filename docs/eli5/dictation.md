# Local dictation

Dictation starts **off**, including when upgrading from settings that predate it.
The Model dropdown in Settings → General → Clipboard & Dictation offers
**Parakeet Unified English** (the default) and **Parakeet-TDT v3**. Choosing one
stops dictation and leaves it off. If it needs a download, Vorkspace asks first,
naming that model, its size (about 731 MB for Unified or 740 MB for TDT), and
where it will be saved. Choose Download or Cancel. Cancelling keeps your new
selection with dictation off. An installed, verified model is reused. After
changing models, switch Local dictation on when ready.

Switching Local dictation on also checks the selected model and asks to download
it if needed. Dictation stays off until that download is verified. Nothing
downloads the other model automatically. Then click Set up permissions. Allow Microphone and
Accessibility for **vorkspace Clipboard** in macOS Privacy & Security settings.
That is Vorkspace's small background helper, which also keeps clipboard history.

The Microphone dropdown uses the Mac's **built-in microphone** by default, including
the internal mic on MacBooks. You can choose **System default** or a specific
connected microphone instead. The list refreshes automatically. If your Mac has
no built-in input, choose another option. A saved microphone that is disconnected
stays marked unavailable; dictation asks you to connect it or choose another.
It never quietly picks a different input. Your choice is saved and still applies
after quitting or reopening Vorkspace.

A microphone change applies to the **next recording**. If the current input is
lost or interrupted, recording stops and its audio is discarded. The helper finishes
setting up the chosen microphone and checks that it is ready before accepting a
recording; setup notices are not treated as an interrupted recording. No microphone
records while dictation is idle. Microphone and Accessibility permission still
belong to the background helper.

Click the field you want to write into. Press **⌘,**, speak, then press **⌘,**
again. A red microphone at the notch means recording. Amber with dots means the
Mac is turning speech into text. Leave the original field selected until the
words arrive. Switching fields discards the words, including switching away and
back. Repeated macOS focus notices do not discard words when the same field is
still selected; a changed destination or a failed check requires a new recording. Pressing the
shortcut during transcription cancels it; Settings also has Cancel. Recording has
a two-minute limit. Vox uses **⌘.**. Settings is available from the application menu.

Everything is transcribed on your Mac. Downloading the model needs internet;
recording does not. Audio stays in memory temporarily and is never saved. There
is no dictation history. Long recordings are handled in short pieces, which may
affect wording and punctuation. Password fields cannot receive dictation.
Ordinary Mac, browser and Vorkspace text boxes use the same Paste action. Some
custom apps hide their text boxes from macOS; dictation then checks the app and
window and watches for other input. Avoid typing, clicking or scrolling until
insertion finishes. It never brings an app forward or clicks a field for you.
If an app secretly moves its own cursor without telling macOS, that change cannot
always be detected. The app must also support Paste; dictation cannot confirm
that it accepted the words.

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

The model and its temporary downloads live in **~/.vorkspace/dictation/**. Moving
the app does not change this folder. All your Vorkspace installs share it, and
only one can download or clean up unfinished downloads for the same model at
a time. Both models can stay installed together; downloading one leaves the
other alone. Other installs
can still read a verified model. If one is busy, wait and retry; an already
verified model will not be downloaded again. If Vorkspace cannot find your home
folder, it explains the problem instead of saving the model elsewhere.

Nothing downloads on startup or before you choose Download. A slow connection to
the background helper keeps your request waiting. Cancel stops the acquisition;
its worker finishes cleanup before another download can start. Replies from that
cancelled request cannot turn dictation on later. No old cache is moved automatically.

Your model choice is saved and sent to the background helper, so it still applies
after quitting or reopening the main app. Changing models discards old dictation
work. Old download messages cannot mark the new model ready or turn it on. TDT is
available as an option; this does not claim Vorkspace has qualified its speed or
transcription quality.

The helper tells its audio engine which microphone to use, then checks that the
same microphone stays selected. It discards a recording if the device changes,
permission is lost, or the engine is interrupted.

Development builds also have a short microphone check. It runs for 2.5 seconds,
reports device and buffer counts, and immediately discards audio without saving
or turning it into words. It does not paste, need Accessibility access, or take
over dictation shortcuts. macOS Microphone permission is still required; the
check reports when that permission is missing. Developers can explicitly ask
macOS to show the permission prompt, which still requires your Allow.
