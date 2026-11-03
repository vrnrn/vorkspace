# Native macOS updates

Stable releases publish a signed Sparkle ZIP and appcast for macOS 26 and later. The release also carries the existing signed legacy JSON manifest pinned to [version 1.1.2](https://github.com/vrnrn/vorkspace/releases/tag/mac-v1.1.2), the macOS 11-compatible bridge for older updater clients. Once installed, the bridge can use Sparkle, which checks the appcast's minimum system version. The app opens existing databases and settings in place. See [data continuity](data-storage.md).

Local builds without Sparkle open the release page when **Check for Updates** is selected. Development-data bundles cannot use a release feed. Packaging prepares files locally.

The manual **Mac release** workflow uses `MACOS_SPARKLE_SIGNING_KEY` with the seed matching `scripts/release/sparkle-public-key.txt`. It reserves an unused version and numeric build number, verifies the Sparkle archive and the appcast's macOS minimum, and copies `scripts/release/legacy-macos-update.json` into the published assets. Keep the bridge release and its signed archive available at the URLs in that manifest.

Release preparation writes version-matched notes to `build/release-notes.md`. The native build embeds the complete notes and rejects release-workflow builds with missing, mismatched or oversized notes. An ordinary local build without bundled notes loads the matching published release body when **What's new** opens, using a bounded request to the release endpoint.

The desktop persists its last observed application version in the existing settings store. A version change produces one installed-update notice across windows and subsequent launches, including updates installed by Sparkle. A first launch without an earlier version or a matching installed-update marker stays quiet. Historical installed-update markers are consumed once; they are not required for subsequent native updates.

## Building an update-enabled app

Download the pinned, checksum-checked [Sparkle 2.9.6 distribution](https://github.com/sparkle-project/Sparkle/releases/tag/2.9.6):

```sh
python3 scripts/release/prepare-native-updates.py
VORKSPACE_SPARKLE_FRAMEWORK="$PWD/target/sparkle/Sparkle.framework" npm run build
```

The bundle declares macOS 26.0 as its minimum. Update packaging checks that the generated Sparkle item declares the same minimum. Bundle assembly checks that embedded frameworks support the bundle's deployment target.

Providing the framework enables the stable HTTPS appcast at `https://github.com/vrnrn/vorkspace/releases/latest/download/appcast.xml` and the public key in `scripts/release/sparkle-public-key.txt`. Its adjacent `LICENSE` is included in the app. Custom feeds may override `VORKSPACE_UPDATE_FEED` and `VORKSPACE_UPDATE_PUBLIC_KEY`; feed URLs must use HTTPS. `VORKSPACE_BUILD_NUMBER` optionally supplies a numeric build number, which must increase between native updates. The default is the application version.

Sparkle and minisign provide open-source signature verification. Ad-hoc application signing is supported; no Apple developer subscription or notarization is required by the packaging helper. For Apple distribution signing, set `VORKSPACE_SIGNING_IDENTITY` to a Developer ID Application identity. The packager signs nested executables inside out and verifies the bundle. macOS trust and privacy prompts still depend on the application's signing identity and distribution method.

## Preparing a Sparkle update

Keep private signing keys outside the repository, owned by the current user with owner-only permissions. The Sparkle key file contains the base64 Ed25519 seed accepted by Sparkle's tools.

```sh
python3 scripts/release/package-native-update.py \
  target/release/vorkspace.app /path/to/native-update-archives \
  --ed-key-file /protected/sparkle.key \
  --download-url-prefix https://github.com/vrnrn/vorkspace/releases/download/mac-v1.0.0/
```

This verifies the application signature, creates and verifies the signed ZIP, and writes the appcast. If `--ed-key-file` is omitted, Sparkle uses its existing Keychain key. Compatibility packaging flags apply only to historical bundles that declare support for macOS 11.0.

Notarization is optional: `--notary-profile YOUR_KEYCHAIN_PROFILE` additionally requires Developer ID signing, submits the bundle to Apple, and staples and validates the ticket before packaging. Omitting that flag avoids Apple's service entirely.

Keep prior native archives and the existing appcast in the output directory for later versions. Archive names are never overwritten. Publish the verified files to the configured release endpoints through the release workflow.

The bundle nests `vorkspace helper.app`, which is signed before the main bundle. After an update, the app replaces the helper still running from the old build, which clears clipboard history.

Sparkle owns native update verification, installation and relaunch. Before distributing a release, test the published old application against the candidate using disposable copies and data. Verify rejected signatures, replacement and relaunch, restored settings and drafts, and a subsequent native update. A local archive test does not prove the hosted feed is available.
