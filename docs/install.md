# Installing a native build

Build from source using [building](building.md). On macOS, `npm run build` creates `target/release/vorkspace.app` and installs it directly to `/Applications/vorkspace.app`. All builds use existing desktop data by default, keeping sessions and project continuity persistent across builds.

With **Keep recent copies** on, the app also runs a small clipboard helper from inside its bundle. It keeps recording after you quit vorkspace, until you log out, use Quit in the clipboard menu, or turn the setting off. It doesn't start at login: after logging in, it starts when you next open the app. Before deleting the app, use Quit in the clipboard menu, or the helper keeps running until you log out.

Published updates use the installed app's usual updater. Release preparation provides the existing desktop download format alongside native Sparkle updates. Both replace the normal app bundle and retain its saved data. See [updates](updates.md) for signing and feed configuration.

Local builds are ad-hoc signed by default. Sparkle is enabled by supplying its framework during release packaging. Developer ID signing and notarization are optional. Building alone never publishes a release.

Release launches use existing desktop data. Isolated development data is available only when explicitly requested via `--dev-data`. See [data continuity](data-storage.md) before choosing `--vorkspace-db` or overriding the development directory.

The desktop keeps the rail, with the vorkspace switcher, Bots and the vorkspace's sessions, and the project's Sessions, Explorer, and Changes sidebar on the left. Drag the sidebar’s right edge to resize it. Bots have a floating Documents island within their own pane; narrow panes open it from the Bot face or the top-right button. Settings → Appearance controls theme, translucency, and interface scale. The top-bar button beside the rail toggle (⇧⌘B) shows or hides that sidebar, and it stays as you leave it. Projects show as folders in an automatic color; a vorkspace's color and chat background are set in **Manage vorkspaces…**. Each project's **Project settings…** dialog, from the project picker at the top of the sidebar, covers Codebase Memory and notifications; the menu itself keeps project actions such as groups, pinning and archiving.
