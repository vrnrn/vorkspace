# Native updates, in plain English

Unconfigured local builds do not update themselves. Their update button opens the
release page. A native release can include Sparkle, which checks the publisher's
signature before installing an update and restarting the app.

Preparing that release needs an Apple signing identity, notarization credentials,
a Sparkle signing key and an HTTPS feed. The packaging command prepares files
locally and submits the app to Apple for notarization; it does not publish the
release. An actual upgrade from an older installed build still needs testing.

See [release preparation](../updates.md).
