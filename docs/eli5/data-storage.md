# Saved data

Development runs use a separate database. Release runs look for your existing desktop database and open it where it already lives. Old settings and Bot metadata are translated when read; conversation text and agent session IDs stay intact.

Use `--check-data` to check a selected database without writing to it. Bot memories stay in the data folder, and remote history stays on the remote machine.

When an agent adds text, the app saves the changed part of the chat instead of copying all the old tool output again. Older chats switch to this storage when they are first saved successfully. Opening a database just to check it does not change its format.

Saved tool details stay intact even when the screen shows only a short preview. If another window changes the history first, the app asks you to reopen it rather than overwrite that change. A failed save leaves the previous saved chat and message receipts together. The remote host follows the same rules.

After the remote host saves a chat in separate blocks, keep using host software that understands that format. Older host software that expects one large saved chat cannot open the converted data. Disconnecting your desktop does not affect the saved chat or stop the host.
