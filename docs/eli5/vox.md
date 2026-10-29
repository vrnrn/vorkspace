# Vox

Vox is the optional voice face. Turn it on in Settings. Its face is hidden from the title bar for now, so you can't start a call from the window yet; once it's back, click it to start a call. It starts disabled. Stop ends the call; silent mode shows replies without speaking them.

Hiding the app or minimizing its window ends the call. Switching chats or projects, opening Settings, or using another app keeps it connected. You start the next call yourself when ready.

You can select and copy a silent reply. Escape or clicking outside hides it until Vox finishes another reply. Very long replies show an ellipsis instead of filling memory with the full text; the full conversation stays with the provider.

Vox uses your selected microphone and keeps its conversation and memories between calls. It can help with local sessions and Bots, while the native app checks and carries out its requests. Reset clears the conversation without letting an older reply bring it back.

Rust manages the call and checks every request. A small private browser surface handles only microphone and sound, keeping the existing Codex sign-in. End releases the microphone immediately and asks Codex to finish saving the conversation. Closing the window also lets an active call finish saving. If that save cannot be confirmed, Vox says so. A failed reconnection keeps the saved conversation available instead of silently replacing it.

The small audio page lives in `embedded/vox`; Rust owns the call and any actions it takes in the app.
