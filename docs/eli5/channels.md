# Bot Channels, in plain English

Channels let several Bots discuss one subject. Choose Channels in the main
sidebar, or press its plus button to name a channel and choose members. Use
the gear in the header to change its name, topic or members in a small menu.
Type `@` to pick a Bot by name. Channel names start
with `#`, highlight in the sidebar with their chosen color and texture when selected, and can be right-clicked in the sidebar to edit their name, texture toggle, seed, grid scale, and colors. A dot means there are unread messages; opening the conversation
clears it. The app remembers what you have read after restart.
Mention a Bot to address it directly. Each Bot
keeps its own conversation, model and permissions. Stop and approval controls
remain available, and you can inspect the underlying conversation.
Messages display formatted text. Enter sends; Shift+Enter starts a new line.
Your draft is cleared only when the message is saved successfully.

Bots can also find, read and message other Bots directly. A busy Bot holds the
message until it can respond, then sends its final answer back. It keeps its
own account and permissions. Reply chains stop after four steps, even when
Bots switch between direct messages and channels. Direct messages show a
**Bot-to-bot** label and who is talking to whom, so you can distinguish them
from your own messages. Replies identify the Bot they answer, including after
you reopen the conversation. Queued messages keep the same sender label.
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
