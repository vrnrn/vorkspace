# Building vorkspace

Install Rust, Node.js 24 or newer, and Xcode on your Mac. Run `npm run dev` to build and open vorkspace with release performance and your normal saved data. Type `r` then Enter in its terminal to rebuild and reopen it. Both build profiles let the runner close the app normally before reopening it. If the new app cannot be installed, the runner opens the fresh build from its output folder. For a separate development copy with isolated data, run `npm run dev -- --native-dev --dev-data`.

You can also use vorkspace to work on vorkspace itself: run `npm run build`, confirm it installed the new app, then choose **Restart vorkspace** or press `⌘⌥R`. Restart uses the app's normal shutdown and saved state, and waits for the old app to exit before opening the new one. It does not rebuild the code. Use the terminal's `r` for a separate development copy, because Restart prefers the installed app on a Mac.

Run `npm ci` and `npm run check` to install test tools and check the code. `npm run assets` rebuilds the editor and voice JavaScript helpers inside the native app. On a Mac, `npm run build` makes `target/release/vorkspace.app` and installs it in `/Applications/`. Set `VORKSPACE_NO_INSTALL=1` to build without installing. Building does not publish a release.

The Rust workspace contains the desktop window and its shared core.

The small voice audio helper is embedded in the core library. Rust runs the voice controller and desktop Copilot connection, so those components need no separate Node process.

Developer tools live under `scripts/`. The small JavaScript components that ship inside the app have their own `embedded/` folder.

The **Mac release** workflow is started manually from `main`. It picks a free version, numbers the build, includes that version's release notes, runs checks, signs the downloads, and publishes only after those steps succeed. Normal code pushes do not publish anything. The app can show its included notes offline and announces a newly installed version once. If you open notes that were not included in the app, it tries to read that exact version's published notes without blocking the window.
