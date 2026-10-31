# Checking changes

Run `npm ci` and `npm run check` for the automated checks, or `npm run check:fast` for a quick local check. Rebuild the embedded helpers with `npm run assets`. Test launch, save, quit, and reopen using temporary development data.

Update tests use disposable copies of the published older app and saved data. They exercise its real installer, restart the new app, check preserved settings and drafts, and reject damaged downloads. Another native update checks Sparkle. These checks can run locally without publishing anything. Draft tests also cover typing during startup and keeping both versions when two windows edit the same unfinished message.

The SSH test starts disposable local services. Microphones, signed updates, and provider sign-in need real end-to-end checks; passing unit tests does not prove those experiences.

Even quick test builds use a fully optimized copy of the small JavaScript engine behind editor highlighting. Its time limits assume the speed of the shipped app, so a slower copy would fail those tests for the wrong reason.

Automated checks use pretend provider processes to test voice recovery, Stop, and duplicate requests without opening a microphone. Copilot translation is checked with a local server. Real accounts and hardware still need a live call.

The startup benchmark under `scripts/benchmarks/` makes pretend history in a temporary folder. It can compare an older commit with the current code, using a fresh process for each measurement. This checks one part of startup, not every part of the app's responsiveness.
