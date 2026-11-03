# Native updates, in plain English

The current app requires macOS 26. The original updater can install version 1.1.2, which works on macOS 11 and later. That version includes Sparkle, which checks the computer's macOS version and the publisher's signature before offering a newer update. The app opens saved data where it already lives.

The manually started **Mac release** workflow signs the new download, checks that it advertises the correct macOS minimum, and keeps the original updater pointed at 1.1.2. Local builds without Sparkle open the release page. Development builds keep separate data.

Published builds carry their own release notes. Local builds can fetch the matching published notes when you open **What's new**. The app remembers which version you last opened and shows an update notice once when that version changes. Opening a fresh installation does not show an update notice unless the older updater left a matching marker.

See [release preparation](../updates.md).

Updating also replaces the small clipboard helper, so clipboard history starts empty after an update.

The background app is named **vorkspace helper**. It owns clipboard history and dictation.
