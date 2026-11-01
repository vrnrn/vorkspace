# Installing vorkspace

Existing users update with their usual update button once a release is published. Their saved data stays in place.

To build your own copy, run `npm run build`. On macOS, this packages the release build and updates `/Applications/vorkspace.app` directly.

The app opens your saved desktop data by default, keeping your sessions and projects persistent. If you ever need a separate sandbox database, pass `--dev-data`. [Data storage](data-storage.md) explains those locations.

Pick a project on the left and work in the middle. Sessions, Explorer, Changes, and Bot details appear on the right. Drag the sidebar's left edge to resize it; it keeps that width when you switch between a session and a Bot. You can change the colors, transparency, and interface size in **Settings → Appearance**.

Building does not publish the app. [Updates](updates.md) explains how to include the open-source updater and prepare signed downloads; paid Apple services are optional.

If clipboard history is on, a small helper keeps it running after you quit vorkspace. Choose Quit in the clipboard menu to stop everything, and do that before deleting the app. After you log out, the helper stays off until you open vorkspace again.
