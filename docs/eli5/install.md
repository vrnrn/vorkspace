# Installing vorkspace

Existing users update with their usual update button once a release is published. Their saved data stays in place.

To build your own copy, run `npm run build`. On macOS, this packages the release build and updates `/Applications/vorkspace.app` directly.

The app opens your saved desktop data by default, keeping your sessions and projects persistent. If you ever need a separate sandbox database, pass `--dev-data`. [Data storage](data-storage.md) explains those locations.

Pick a vorkspace at the top left and a project at the top of the sidebar, then work in the middle. Sessions, Explorer and Changes stay on the left; drag the sidebar’s right edge to resize it. A Bot’s documents appear in a floating island on its right; the sessions it starts are listed on the left. In a narrow pane, click the Bot face or the small top-right button to show it. You can change the colors, transparency, and interface size in **Settings → Appearance**. To change one project's name, logo, colors, background picture or notifications, choose **Project settings…** from the project picker.

Building does not publish the app. [Updates](updates.md) explains how to include the open-source updater and prepare signed downloads; paid Apple services are optional.

If clipboard history is on, a small helper keeps it running after you quit vorkspace. Choose Quit in the clipboard menu to stop everything, and do that before deleting the app. After you log out, the helper stays off until you open vorkspace again.
