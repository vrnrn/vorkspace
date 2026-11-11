# Checking changes

Run `npm ci` and `npm run check` for the automated checks; the Rust tests need `cargo-nextest` installed once and run side by side to finish quickly. Run `npm run check:fast` for a quick local check. Rebuild the embedded helpers with `npm run assets`. Test launch, save, quit, and reopen using temporary development data.

Database tests, screenshots, and the benchmark create disposable data using the same database layout as the app. They can close and reopen it, including its settings file, without using your saved chats or personal settings. Checks of older saved formats also open temporary copies through the app's normal database entry points.

Update tests use disposable copies of the published older app and saved data. They exercise its real installer, restart the new app, check preserved settings and drafts, and reject damaged downloads. Another native update checks Sparkle. These checks can run locally without publishing anything. Draft tests also cover typing during startup and keeping both versions when two windows edit the same unfinished message.

Microphones, signed updates, and provider sign-in need real end-to-end checks; passing unit tests does not prove those experiences.

Even quick test builds use a fully optimized copy of the small JavaScript engine behind editor highlighting. Its time limits assume the speed of the shipped app, so a slower copy would fail those tests for the wrong reason.

Automated checks use pretend provider processes to test voice recovery, Stop, and duplicate requests without opening a microphone. Copilot translation is checked with a local server. Real accounts and hardware still need a live call.

`npm run benchmark:native` times the real app drawing into an invisible window, using a big pretend history. It shows how long opening a long chat takes and how long the app spends working out each frame, so you can compare before and after a change. Most of the numbers stop before the graphics card; one, `metal_render_readback`, includes it. None of them run a live streaming reply.

`npm run check` also tests clipboard history by starting **vorkspace helper** with its own private connection, separate from any app you have open. Another check fails if a guide mentions an `npm run` command that doesn't exist, or if a guide loses its plain-English partner.
