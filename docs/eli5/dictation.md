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
