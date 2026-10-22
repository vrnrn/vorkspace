# Installing a native build

Build from source using [building](building.md). On macOS, `npm run build` creates `target/release/vorkspace.app`. Copy that bundle to `~/Applications/` when you choose to replace your installed application. Quit a running copy first; multiple processes must not compete for microphone or hotkey ownership.

Local builds are ad hoc signed by default. Developer ID signing, notarization, and Sparkle require the explicit configuration described in [updates](updates.md). Building alone neither publishes a release nor enables an update feed.

Release launches use existing desktop data. Development launches use isolated data. See [data continuity](data-storage.md) before choosing `--vorkspace-db` or overriding the development directory.
