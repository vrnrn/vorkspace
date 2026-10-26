# Architecture

The desktop app window is drawn by GPUI. Rust keeps track of conversations, files, Bots, and running agents in the same desktop process. Saved history lives in SQLite, with extra window settings beside it.

A Bot's face and name stay at the top of its chat as messages scroll. Its face shows whether it is idle, working or needs you, without following the mouse. On Mac, messages fade gently around the face to reveal the chat's own background colors and picture. You can still select, click and scroll the chat there, and the fade stays above the typing box. Hidden or covered faces stop animating, and an empty chat shows help instead of a second face.

Open Bots in the main app window. Click the macOS menu bar icon or press ⌘⇧C to find recently copied text and images. Search finds text; image thumbnails show what you copied. Hover over an image for one second to show its larger preview; moving away hides that pane. Choosing an item puts its original contents back on the clipboard, ready for your usual Paste command. Settings and Quick Composer are also in this menu.

Clipboard history keeps at most 50 items in memory and disappears when you quit. It also has a memory limit, so large images can make older items leave sooner. Very large copies are skipped. Clear history removes the app's saved copies; it leaves your current system clipboard alone. Turning clipboard history off in the Clipboard group in General settings clears it and stops watching for copies. When history is off, the menu shows only Settings, Quick Composer and Quit. Password-manager copies marked private are ignored.

The app checks whether the clipboard changed twice a second. It makes small image previews in the background, one at a time, and never saves a clipboard history file.

Rust controls Vox calls and translates desktop Copilot requests. Small JavaScript helpers handle editor language tools, voice audio, and remote machines. Empty conversations show the composer, and session titles stay still. A remote machine keeps its own agents running and history saved even when your desktop disconnects. Voice and HTML previews use a limited browser surface inside the native app.

The [repository map](repository.md) shows where each part lives. Embedded helper code has its own folder; the scripts folder contains developer tools.

Long saved chats are read a piece at a time. Hidden views keep their work without repeatedly asking to redraw, and repeated terminal redraw requests are combined. Cleaning up one idle agent does not hold the shared lock that other agents need.

Every click of Terminal in the sidebar opens and focuses a new terminal across the main workspace, starting in your selected project or worktree folder. You can keep several independent shells in the same folder. Click a terminal tab at the top to return to that exact shell. Switching tabs keeps its output and commands running; selecting a different folder never moves an existing shell. Closing its tab ends only that shell, with a warning if a command is running. After quitting and reopening the app, saved terminal tabs start new shells in their saved starting folders when selected. Running programs and old terminal output do not survive quitting. The smaller terminal dock and its shortcuts still work separately.

As a reply arrives, only changed chat blocks are saved. The screen can show a small preview while the full tool result stays safely in the database. A stale window cannot overwrite newer saved history.

Bot cards save in the background, one card at a time. Repeated updates combine, and an unchanged card needs no database work. Saving a card leaves the rest of the conversation untouched. The screen changes after the save succeeds, provided it still shows the same card and conversation. An old reply cannot replace a newer card. Closing its view does not cancel a save. Quitting waits for admitted card saves, which can delay shutdown if storage is busy. A forced stop can lose a refresh that has not yet saved or appeared on screen.

Every supported agent has an output size limit before it is buffered. A fast agent waits when the app is still handling earlier output. An individual response that is too large produces a clear error rather than silently losing part of its text. Stopping the connection also releases waiting readers.

Interface zoom makes the whole workspace larger or smaller, from 60% to 160%. Clicking, typing, native previews, and screen-reader controls follow the same zoom level, including after moving the window to a different display.
