# Building vorkspace

Install Rust, Node.js 24 or newer, and Xcode on your Mac. Run `npm run dev` to open a development copy with separate data. Type `r` then Enter in its terminal to rebuild it.

Run `npm ci` and `npm run check` to install test tools and check the code. `npm run assets` rebuilds the small JavaScript helpers inside the native app. `npm run build` makes `target/release/vorkspace.app`; that release copy uses your normal saved data.

The Rust workspace contains the desktop window and its shared core.

The small voice audio helper is embedded in the core library. Rust runs the voice controller and desktop Copilot connection, so those components need no separate Node process.

Developer tools live under `scripts/`. The small JavaScript components that ship inside the app have their own `embedded/` folder.
