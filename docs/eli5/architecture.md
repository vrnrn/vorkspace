# Architecture

The desktop app window is drawn by GPUI. Rust keeps track of conversations, files, Bots, and running agents in the same desktop process. Saved history lives in SQLite, with extra window settings beside it.

Rust controls Vox calls and translates desktop Copilot requests. Small JavaScript helpers handle editor language tools, voice audio, and remote machines. Empty conversations show the composer, and session titles stay still. A remote machine keeps its own agents running and history saved even when your desktop disconnects. Voice and HTML previews use a limited browser surface inside the native app.

The [repository map](repository.md) shows where each part lives. Embedded helper code has its own folder; the scripts folder contains developer tools.

Long saved chats are read a piece at a time. Hidden views keep their work without repeatedly asking to redraw, and repeated terminal redraw requests are combined. Cleaning up one idle agent does not hold the shared lock that other agents need.

As a reply arrives, only changed chat blocks are saved. The screen can show a small preview while the full tool result stays safely in the database. A stale window cannot overwrite newer saved history.

Agent output has a size limit before it is buffered. A fast agent waits when the app is still handling earlier output. An individual response that is too large produces a clear error rather than silently losing part of its text. Stopping the connection also releases waiting readers.
