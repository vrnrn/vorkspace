# Native macOS updates

The release packager prepares two standard, signed update formats: the installed desktop client's `.app.tar.gz` and JSON feed, and Sparkle's ZIP and appcast for native builds. Both install the normal application bundle. There is no transition app or data-moving installer. Existing databases and settings are opened in place; see [data continuity](data-storage.md).

Local builds without Sparkle open the release page when **Check for updates** is selected. Development-data bundles cannot use a release feed. Packaging never uploads files or publishes a release.

## Building an update-enabled app

Download the pinned, checksum-checked [Sparkle 2.9.6 distribution](https://github.com/sparkle-project/Sparkle/releases/tag/2.9.6) for macOS 11 compatibility:

```sh
python3 scripts/release/prepare-native-updates.py
VORKSPACE_SPARKLE_FRAMEWORK="$PWD/target/sparkle/Sparkle.framework" npm run build
```

The app retains an 11.0 deployment minimum; packaging rejects frameworks that require a newer macOS version. Sparkle 2.10 raises its minimum to 12.0, so the pinned 2.9.6 distribution preserves the older app’s supported installation range.

Providing the framework enables the stable HTTPS appcast at `https://github.com/vrnrn/vorkspace/releases/latest/download/appcast.xml` and the public key in `scripts/release/sparkle-public-key.txt`. Its adjacent `LICENSE` is included in the app. Custom feeds may override `VORKSPACE_UPDATE_FEED` and `VORKSPACE_UPDATE_PUBLIC_KEY`; feed URLs must use HTTPS. `VORKSPACE_BUILD_NUMBER` optionally supplies a numeric build number, which must increase between native updates. The default is the application version.

Sparkle and minisign provide open-source signature verification. Ad-hoc application signing is supported; no Apple developer subscription or notarization is required by the packaging helper. For Apple distribution signing, set `VORKSPACE_SIGNING_IDENTITY` to a Developer ID Application identity. The packager signs nested executables inside out and verifies the bundle. macOS trust and privacy prompts still depend on the application's signing identity and distribution method.

## Preparing both update formats

Keep private signing keys outside the repository, owned by the current user with owner-only permissions. The Sparkle key file contains the base64 Ed25519 seed accepted by Sparkle's tools. The existing desktop key and public key accept standard minisign text or their existing outer-base64 encoding. Install the open-source `minisign` command to prepare that format.

```sh
python3 scripts/release/package-native-update.py \
  target/release/vorkspace.app /path/to/native-update-archives \
  --ed-key-file /protected/sparkle.key \
  --compat-signing-key /protected/desktop.key \
  --compat-public-key /path/to/desktop.key.pub \
  --download-url-prefix https://github.com/vrnrn/vorkspace/releases/download/mac-v1.0.0/
```

This verifies the application signature, creates and signs both archives, verifies their signatures, and writes `appcast.xml`, `.sig`, and `latest.json`. The JSON feed retains the installed clients' platform names, including the `-app` aliases. The compatibility flags must be supplied together; omit both when preparing only native updates. The default desktop-key password is empty; `--compat-password-stdin` reads one line from stdin instead. Private key contents are never passed as command arguments. If `--ed-key-file` is omitted, Sparkle uses its existing Keychain key.

Notarization is optional: `--notary-profile YOUR_KEYCHAIN_PROFILE` additionally requires Developer ID signing, submits the bundle to Apple, and staples and validates the ticket before packaging. Omitting that flag avoids Apple's service entirely.

Keep prior native archives and the existing appcast in the output directory for later versions. Archive names are never overwritten. Publishing the verified files to the configured release endpoints is a separate action; retain the existing desktop signing key so installed clients can verify the first native update.

Sparkle owns native update verification, installation and relaunch. Before distributing a release, test the published old application against the candidate using disposable copies and data. Verify rejected signatures, replacement and relaunch, restored settings and drafts, and a subsequent native update. A local archive test does not prove the hosted feed is available.
