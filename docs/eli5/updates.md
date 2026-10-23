# Native updates, in plain English

People with the older app can receive the new app through their usual update button. Release preparation makes both the older app's signed download and Sparkle's signed download. The app opens saved data where it already lives; no special moving tool is needed.

A native release includes the open-source Sparkle updater. The pinned version keeps macOS 11 compatibility, and packaging rejects a framework that would silently require a newer system. It checks the publisher's signature before installing an update and restarting. Unconfigured local builds open the release page instead, and development builds keep separate data.

Preparing updates uses private signing keys kept outside this repository. Apple's paid signing and notarization services are optional. Preparing files does not publish them. Test a real old-to-new update and another native update with disposable data before publishing, including rejection of damaged downloads and recovery of settings and unfinished messages.

Publishing uses the manually started **Mac release** workflow and its two protected signing secrets. It keeps the existing public keys so installed users trust the downloads, chooses an unused version, and publishes after checks pass. Regular pushes and local tests never start that workflow.

See [release preparation](../updates.md).
