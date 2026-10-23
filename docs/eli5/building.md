# Building vorkspace

Install Rust, Node.js 24 or newer, and Xcode on your Mac. Run `npm run dev` to open a development copy with separate data. Type `r` then Enter in its terminal to rebuild it.

Run `npm ci` and `npm run check` to install test tools and check the code. `npm run assets` rebuilds the editor and voice JavaScript helpers inside the native app. `npm run build` makes `target/release/vorkspace.app`; that release copy uses your normal saved data.

The Rust workspace contains the desktop window and its shared core.

The small voice audio helper is embedded in the core library. Rust runs the voice controller and desktop Copilot connection, so those components need no separate Node process.

Developer tools live under `scripts/`. The small JavaScript components that ship inside the app have their own `embedded/` folder.

The **Mac release** workflow is started manually from `main`. It picks a free version, numbers the build, includes that version's release notes, runs checks, signs the downloads, and publishes only after those steps succeed. Normal code pushes do not publish anything. The app can show its included notes offline and announces a newly installed version once. If you open notes that were not included in the app, it tries to read that exact version's published notes without blocking the window.
