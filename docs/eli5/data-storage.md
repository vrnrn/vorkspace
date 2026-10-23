# Saved data

Development runs use a separate database. Release runs look for your existing desktop database and open it where it already lives. Old settings and Bot metadata are translated when read; conversation text and agent session IDs stay intact.

Use `--check-data` to check a selected database without writing to it. Unsent messages, cursor selections, attachments and MCP tags reopen from the same saved draft. Edits are saved while typing and flushed when the view closes or the app quits. If two windows edit the same draft, neither silently overwrites the other. The conflicting input stays saved separately and appears with a preview when you reopen the chat, so you can choose which version to keep. Missing attachments stay recorded so sending cannot silently drop them. Vox also keeps its saved enabled, silent and microphone choices.

Bot memories stay in the data folder where they were saved, even when an earlier version used a different folder name. The app keeps using that same saved owner when you edit it. Empty newer folders do not hide older memories, and remote history stays on the remote machine.

When an agent adds text, the app saves the changed part of the chat instead of copying all the old tool output again. Older chats switch to this storage when they are first saved successfully. Opening a database just to check it does not change its format.

Saved tool details stay intact even when the screen shows only a short preview. If another window changes the history first, the app asks you to reopen it rather than overwrite that change. A failed save leaves the previous saved chat and message receipts together. The remote host follows the same rules.

After the remote host saves a chat in separate blocks, keep using host software that understands that format. Older host software that expects one large saved chat cannot open the converted data. Disconnecting your desktop does not affect the saved chat or stop the host.

Before sending locally, the app saves a record of the message. If the connection breaks or the app stops, it does not assume the agent did nothing and send the message again. Unconfirmed messages stay available to review. You can explicitly put a copy back in an empty composer. Dismissing the notice does not permit an automatic repeat.

Pasted pictures stay in a private attachment folder. The app makes sure a file is saved before sharing its path. A background check can remove old pictures that no saved message or draft uses. If another app process is using the folder, saved data changes, or the check cannot finish safely, it leaves the pictures alone. A read-only data check never cleans attachments.
