# Vox

Vox is an optional voice assistant that appears around your Mac's notch. Turn it on in Settings, then press Control Option Space from any app, or click the microphone above Subscriptions in Vorkspace. You can change the shortcut in Keyboard Shortcuts. Vox starts disabled.

Its face appears on the left of the notch and its status appears on the right. Silent mode opens the reply below the notch without reading it aloud. On a screen without a notch, Vox appears at the top center.

You can keep using other apps, hide Vorkspace, or minimize its main window while talking. After 30 seconds without activity, Vox releases the microphone and tucks itself away. It waits while you are talking, while it is working or speaking, or while you are interacting with the reply. Click Stop, press the shortcut again, or press Escape in the panel to end the call yourself.

You can select and copy a silent reply. Dismiss hides it until Vox finishes another reply. Escape ends the call. Very long replies show an ellipsis instead of filling memory with the full text; the full conversation stays with the provider.

Vox uses your selected microphone and keeps its conversation and memories between calls. It can help with local sessions and Bots, while the native app checks and carries out its requests. Reset clears the conversation without letting an older reply bring it back.

Rust manages the call and checks every request. A small private browser surface handles only microphone and sound, keeping the existing Codex sign-in. End releases the microphone immediately and asks Codex to finish saving the conversation. Closing the window also lets an active call finish saving. If that save cannot be confirmed, Vox says so. A failed reconnection keeps the saved conversation available instead of silently replacing it.

The small audio page lives in `embedded/vox`; Rust owns the call and any actions it takes in the app.
