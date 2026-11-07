# Building vorkspace

Install Rust through `rustup`, Zig 0.17.x, Node.js 24 or newer, and Xcode with the macOS 26 SDK on a Mac running macOS 26 or newer. Also check out fluidither, the dotted, springy look the app borrows, in a folder named `fluidither-gpui-pre` next to this one. To run the tests, also install `cargo-nextest` once with `cargo install --locked cargo-nextest`. Run `npm run dev` to build and open vorkspace with your normal saved data. It uses a quick-to-rebuild version that runs a little slower than the real app and stays out of your Applications folder; add `--release` to get the real thing. Type `r` then Enter in its terminal to rebuild and reopen it. Both build profiles let the runner close the app normally before reopening it. With `--release`, if the new app cannot be installed, the runner opens the fresh build from its output folder. For a separate development copy with isolated data, run `npm run dev -- --native-dev --dev-data`.

You can also use vorkspace to work on vorkspace itself: run `npm run build`, confirm it installed the new app, then choose **Restart vorkspace** or press `⌘⌥R`. Restart uses the app's normal shutdown and saved state, and waits for the old app to exit before opening the new one. It does not rebuild the code. Use the terminal's `r` to rebuild a development copy.

Run `npm ci` and `npm run check` to install test tools and check the code. `npm run assets` rebuilds the editor and voice JavaScript helpers inside the native app. On a Mac, `npm run build` makes `target/release/vorkspace.app` and installs it in `/Applications/`. Set `VORKSPACE_NO_INSTALL=1` to build without installing. Building does not publish a release.

The project selects a specific Rust compiler and its required tools automatically. Builds require macOS 26 throughout, and packaging checks both program files before signing the app.

Automated checks use a faster-to-compile version of the application, while keeping the JavaScript interpreter fast enough for its time limits. This does not change the normal development or release builds. GitHub checks and releases reuse compiled dependencies where possible, but still run all checks. A new compiler or SDK can make the first run slower.

The Rust workspace contains the desktop window and its shared core. Zig builds the included Ghostty terminal engine. People using the finished app do not need to install Zig.

The small voice audio helper is embedded in the core library. Rust runs the voice controller and desktop Copilot connection, so those components need no separate Node process.

Developer tools live under `scripts/`. The small JavaScript components that ship inside the app have their own `embedded/` folder.

The **Mac release** workflow is started manually from `main`. It picks a free version, numbers the build, includes that version's release notes, runs checks, signs the downloads, and publishes only after those steps succeed. Normal code pushes do not publish anything. The app can show its included notes offline and announces a newly installed version once. If you open notes that were not included in the app, it tries to read that exact version's published notes without blocking the window.

Restart always reopens the copy of the app you are using, so a development build stays a development build even if an older app sits in your Applications folder.

Building leaves a lot of files behind, and the Rust tools never throw old ones away on their own. To keep your disk from filling up, the app's build commands tidy up after themselves: they keep the newest couple of copies of each built program and delete the older ones. If you build by hand with `cargo`, run `npm run target:prune` afterwards to do the same. Try not to give each task its own build folder; every extra folder is another full copy of everything.

Starting a new worktree? Run `npm run worktree:seed` first to reuse build files from your main checkout and speed up the first build. To seed new worktrees automatically, set `npm run worktree:seed` as the project's setup command under **Settings → Worktrees**.

Add `--dev-data` to `npm run dev` to use a separate set of practice data. The sample conversations from `--fixture` only work with that option.

**vorkspace helper** handles clipboard history, [dictation](dictation.md) and meeting recordings. Clipboard history and dictation stay available after you close a development build. Rebuilding replaces the helper and clears its clipboard history. Choose Quit in the clipboard menu to stop it. When no note is recording, turning off both **Keep recent copies** and **Dictation** also stops it. For `cargo run`, build `vorkspace-helper` with the same `--profile` first so the app can find it beside its own program file.
