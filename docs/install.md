# Installing a native build

Build from source using [building](building.md). On macOS, `npm run build` creates `target/release/vorkspace.app`. Copy that bundle to `~/Applications/` when you choose to replace your installed application. Quit a running copy first; multiple processes must not compete for microphone or hotkey ownership.

Published updates use the installed app's usual updater. Release preparation provides the existing desktop download format alongside native Sparkle updates. Both replace the normal app bundle and retain its saved data. See [updates](updates.md) for signing and feed configuration.

Local builds are ad-hoc signed by default. Sparkle is enabled by supplying its framework during release packaging. Developer ID signing and notarization are optional. Building alone never publishes a release.

Release launches use existing desktop data. Development launches use isolated data. See [data continuity](data-storage.md) before choosing `--vorkspace-db` or overriding the development directory.

The desktop keeps the familiar project rail on the left and the main workspace in the middle. Sessions, Explorer, and Changes share the right-hand sidebar with Bot details. Both use the same saved width, so resizing either keeps the size consistent when switching between sessions and Bots. Settings → Appearance controls theme, translucency, and interface scale.
