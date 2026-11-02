# Local dictation

Dictation starts **off**, including when upgrading from settings that predate it.
The Model dropdown in Settings → General → Dictation offers
**Parakeet Unified English** (the default), **Parakeet-TDT v3**, and
**Parakeet Flash**. TDT v3 and Flash are experimental. Flash is available on Apple silicon with macOS 14 or newer.
Choosing one
stops dictation and leaves it off. If it needs a download, Vorkspace asks first,
naming that model, its size (about 731 MB for Unified, 740 MB for TDT, or 224 MB
for Flash), and
where it will be saved. Choose Download or Cancel. Cancelling keeps your new
selection with dictation off. An installed, verified model is reused. After
changing models, switch Local dictation on when ready.

Switching Local dictation on also checks the selected model and asks to download
it if needed. Dictation stays off until that download is verified. Nothing
downloads another model automatically. Then click Set up permissions. Allow Microphone and
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

When enabled, Vorkspace prepares the selected model and keeps it ready in memory.
Wait for Ready, click your text field, then press **⌘,**. An amber microphone means startup;
wait for red before speaking. Stable words appear in the
field while you speak. It waits for words to settle instead of inserting guesses
and deleting them later. Press **⌘,** again to finish the last words, without
repeating what is already there. The notch shows a microphone, not your text.
On screens without a notch, the microphone appears by itself at the top center.
The model can miss punctuation at the end of a sentence; Vorkspace does not
guess punctuation that the model did not supply.
Quiet background noise need not hold the last settled word until you speak again.
TDT checks overlapping pieces of speech, so words can arrive in small bursts.
Flash is a fast English option that does not add punctuation or capital letters.
If another updated Vorkspace helper is keeping a dictation model ready, this
helper waits and retries automatically; disable dictation in the other helper to
let it continue. Clipboard history can keep running. An older helper that has
not been updated cannot participate in this coordination.

To teach a name or spelling, open **Custom words**, add a term such as
`Vorkspace` on its own line, then save. If it is misheard, add a line such as
`work space => Vorkspace`. The left side is what the model might
write; the right side is what you want inserted. The correction ignores letter
case, keeps punctuation around the phrase, and works only on whole words. If
two rules overlap, the longer one wins. Your saved rules stay on this Mac and
apply to the next recording, even after you quit the main app.

TDT also uses preferred words as hints while scoring speech; this may help but
does not guarantee recognition. Unified and Flash correct matching text only;
a term on its own can correct its capitalization but cannot fix a different
misheard word unless you add that form. Cancelling leaves
already inserted words alone and discards text that was still waiting.
A trailing word or phrase that could match a correction can wait for a word
boundary or for you to stop so the correction stays safe.

Typing, clicking, navigating, switching apps or fields, losing the microphone,
changing models or cancelling stops later words. Words already inserted stay.
Switching away and back does not restart insertion. Unchanged macOS focus notices
are harmless. Pressing **⌘,** during startup cancels; pressing it while finishing
also cancels. Settings has Cancel too. Recording has a two-minute limit. Vox uses
**⌘.**. Settings is available from the application menu. No microphone is open
while idle, and switching dictation off releases the prepared model after its
worker has stopped.

Everything is transcribed on your Mac. Downloading the model needs internet;
recording does not. Audio stays in memory temporarily and is never saved. There
is no dictation history. Unified uses buffered streaming. TDT checks overlapping
pieces while you speak, which may affect wording and punctuation. Flash uses a
streaming model and keeps listening after natural pauses.
Password fields cannot receive dictation.
Ordinary Mac, browser and Vorkspace text boxes use the same Paste action. Some
custom apps hide their text boxes from macOS; dictation then checks the app and
window and watches for other input. Avoid typing, clicking or scrolling until
insertion finishes. It never brings an app forward or clicks a field for you.
If an app secretly moves its own cursor without telling macOS, that change cannot
always be detected. The app must also support Paste; dictation cannot confirm
that it accepted the words.

Developers can also run public or generated recordings through a debug fixture
with a pretend text destination. It checks that live words and the final suffix
arrive in order without opening a microphone or changing a real text field.

Dictation and clipboard history can be switched on separately. Either keeps the
helper alive when the main app quits. Switch both off, or use Quit in the helper's
menu, to stop it. Vox and dictation take turns using the microphone and notch.
Each app bundle has its own helper, so two bundles can keep two models ready at
once. Voice features sharing the same temporary folder take turns using the
microphone and notch.

For each new stable piece, the helper briefly puts only the new words on the
clipboard and sends Paste once. It checks whether the modifier keys are held and
waits before replacing an outstanding paste. It saves
all the previous clipboard formats and restores them only if you have not copied
something newer. Very large or unreadable clipboards are left alone and insertion
is declined. The helper cannot confirm whether another app accepted Paste, so it
never retries automatically. If something fails, check the message in Settings
and make a new recording. Your previous model survives a failed download; Retry
checks the new model before replacing it.

The model and its temporary downloads live in **~/.vorkspace/dictation/**. Moving
the app does not change this folder. All your Vorkspace installs share it, and
only one can download or clean up unfinished downloads for the same model at
a time. All three models can stay installed together; downloading one leaves the
others alone. Other installs
can still read a verified model. If one is busy, wait and retry; an already
verified model will not be downloaded again. If Vorkspace cannot find your home
folder, it explains the problem instead of saving the model elsewhere.

Nothing downloads on startup or before you choose Download. A slow connection to
the background helper keeps your request waiting. Cancel stops the acquisition;
its worker finishes cleanup before another download can start. Replies from that
cancelled request cannot turn dictation on later. No old cache is moved automatically.

Your model choice is saved and sent to the background helper, so it still applies
after quitting or reopening the main app. Changing models cancels old dictation work and keeps already inserted words. Old download messages cannot mark the new model ready or turn it on. TDT is
available as an option; this does not claim Vorkspace has qualified its speed or
transcription quality.

The helper tells its audio engine which microphone to use, then waits for the
first input and checks that the same microphone stays selected. It discards the
brief startup audio until this check finishes. If input never arrives, it stops
within two seconds. macOS startup notices do not interrupt this preparation; a
notice during the final check or recording still stops later insertion. It stops
a recording if the device changes,
permission is lost, or the engine is interrupted.

Development builds also have a short microphone check. It runs for 2.5 seconds,
reports device and buffer counts, and immediately discards audio without saving
or turning it into words. It does not paste, need Accessibility access, or take
over dictation shortcuts. macOS Microphone permission is still required; the
check reports when that permission is missing. Developers can explicitly ask
macOS to show the permission prompt, which still requires your Allow.

Developer builds can feed a known public recording through the same live worker
twice, measuring words arriving before Stop and checking model reuse. A separate
offline check joins or crashes disposable helper processes to exercise the model
lock and retry, using existing models and no microphone. Another checks the last
word during a noisy pause. A field check uses only synthetic text to test several
pastes, edits, focus changes and cancellation. This requires the signed helper's
normal Accessibility permission; a missing grant is reported without changing
permissions. These checks never use your microphone, composer or database.

A combined developer check can also send the known public recording through the
real dictation controller and watch the disposable text field change while the
recording plays. It checks two turns with one prepared model, keeps the known
starting text, rejects missing or duplicated words, and times when text actually
appears. The disposable app resets only its own test text between turns. This
check requires the helper's normal Accessibility permission and refuses if that
permission is missing; it does not record your microphone or touch your work.
