# Native macOS updates

Development bundles are ad-hoc signed and have no update feed. **Check for
updates** opens the release page when Sparkle is not configured. Packaging does not publish archives or attach an update feed automatically.

The native packager can embed Sparkle 2.10.0. Download the pinned, checksum-checked
distribution with:

```sh
python3 scripts/release/prepare-native-updates.py
```

Configure a release build with all three values together:

- `VORKSPACE_SPARKLE_FRAMEWORK`: absolute path to the downloaded `Sparkle.framework`;
  its adjacent `LICENSE` is included in the app.
- `VORKSPACE_UPDATE_FEED`: the native HTTPS appcast URL.
- `VORKSPACE_UPDATE_PUBLIC_KEY`: the Sparkle Ed25519 public key.

Set `VORKSPACE_SIGNING_IDENTITY` to the Developer ID Application identity and
`VORKSPACE_BUILD_NUMBER` to a monotonically increasing numeric build number.
Then run `npm run build`. The packager signs nested Sparkle executables
inside out and verifies the entire bundle. The app's hardened-runtime
entitlements permit microphone input. Update configuration is refused for
development-data bundles.

With the corresponding Sparkle private key and notarytool credentials already
in Keychain, prepare a distribution without uploading it:

```sh
python3 scripts/release/package-native-update.py \
  "target/release/vorkspace.app" \
  /path/to/native-update-archives \
  --notary-profile YOUR_KEYCHAIN_PROFILE \
  --download-url-prefix https://downloads.example.com/native/
```

This verifies Developer ID signing, submits to Apple for notarization, staples
and validates the ticket, creates a ZIP and runs Sparkle's `generate_appcast`.
Keep previous update archives and the existing appcast in that output directory
when preparing later versions. Existing archive names are never overwritten.
Private keys are not passed on command lines. Publication is a separate action.

Sparkle owns update verification, installation and relaunch. Before distributing
a release, verify an installed old-version → new-version update using disposable
app data, including rejected signatures and restored conversation state. Building
an archive alone does not establish that the hosted feed and installed updater
work together.
