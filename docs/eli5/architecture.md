# Architecture

The desktop app window is drawn by GPUI. Rust keeps track of conversations, files, Bots, and running agents in the same desktop process. Saved history lives in SQLite, with extra window settings beside it.

Open Bots in the main app window. Click the macOS menu bar icon or press ⌘⇧C to find recently copied text and images. Search finds text; image thumbnails show what you copied. Hover over an image for one second to show its larger preview; moving away hides that pane. Choosing an item puts its original contents back on the clipboard, ready for your usual Paste command. Settings and Quick Composer are also in this menu.

Clipboard history keeps at most 50 items in memory and disappears when you quit. It also has a memory limit, so large images can make older items leave sooner. Very large copies are skipped. Clear history removes the app's saved copies; it leaves your current system clipboard alone. Turning clipboard history off in the Clipboard group in General settings clears it and stops watching for copies. When history is off, the menu shows only Settings, Quick Composer and Quit. Password-manager copies marked private are ignored.

The app checks whether the clipboard changed twice a second. It makes small image previews in the background, one at a time, and never saves a clipboard history file.

Rust controls Vox calls and translates desktop Copilot requests. Small JavaScript helpers handle editor language tools, voice audio, and remote machines. Empty conversations show the composer, and session titles stay still. A remote machine keeps its own agents running and history saved even when your desktop disconnects. Voice and HTML previews use a limited browser surface inside the native app.

The [repository map](repository.md) shows where each part lives. Embedded helper code has its own folder; the scripts folder contains developer tools.

Long saved chats are read a piece at a time. Hidden views keep their work without repeatedly asking to redraw, and repeated terminal redraw requests are combined. Cleaning up one idle agent does not hold the shared lock that other agents need.

As a reply arrives, only changed chat blocks are saved. The screen can show a small preview while the full tool result stays safely in the database. A stale window cannot overwrite newer saved history.

Every supported agent has an output size limit before it is buffered. A fast agent waits when the app is still handling earlier output. An individual response that is too large produces a clear error rather than silently losing part of its text. Stopping the connection also releases waiting readers.

Interface zoom makes the whole workspace larger or smaller. Clicking, typing, native previews, and screen-reader controls follow the same zoom level, including after moving the window to a different display.
