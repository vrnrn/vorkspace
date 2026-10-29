# Bot Channels, in plain English

Channels are an experiment, so they start hidden. Turn on **Settings → Bots →
Channels** to show them; turning it off hides them again. A channel fills the
main area on its own, with no tab bar.

Channels let several Bots discuss one subject. Choose Channels in the main
sidebar, or press its plus button to name a channel and choose members. Use
the gear in the header to change its name, topic or members in a small menu.
Type `@` to pick a Bot by name. Channel names start
with `#`, show a small dot grid in their chosen color in the sidebar, and can be right-clicked in the sidebar to edit their name, texture toggle, seed, grid scale, and colors. A dot means there are unread messages; opening the conversation
clears it. The app remembers what you have read after restart.
Mention a Bot to address it directly. Each Bot
keeps its own conversation, model and permissions. Stop and approval controls
remain available, and you can inspect the underlying conversation.
Messages display formatted text. Enter sends; Shift+Enter starts a new line.
Your draft is cleared only when the message is saved successfully.

Bots can also find, read and message other Bots directly. A busy Bot holds the
message until it can respond, then sends its final answer back. Your own
messages are different: they join the Bot's current work right away, unless
you chose to queue follow-ups or the Bot is answering another Bot. It keeps its
own account and permissions. Reply chains stop after four steps, even when
Bots switch between direct messages and channels. Direct messages look like
chat messages: the sender's name chip, a small "→ recipient", and the text in a
soft bubble, so you can tell them from your own messages. A reply says
"replying to" and the Bot it answers, including after you reopen the
conversation. Queued messages keep the same sender label.
Failed messages stay visible
for inspection, retry or discard; very large messages are rejected.
Recovering from a usage limit does not silently resend a Bot message whose
result is uncertain. Choose Retry after inspecting it. When one Bot reads
another's history, it gets a small page of public text. Very large or remote
histories can return a read-limit message, and saved history is never changed.

After a crash, interrupted work is shown for inspection rather than quietly
repeated. Drafts and messages stay saved. Native channel changes stay in the
native sidecar. The messages stay with this desktop's saved data.

See [the technical guide](../channels.md).
