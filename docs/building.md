# Build from source

vorkspace publishes Apple Silicon macOS releases. The source also includes Linux and Windows build targets. You need Node.js 20+ and a current stable Rust toolchain. Install and sign in to at least one agent CLI using [provider setup](providers.md).

## Platform prerequisites

On Linux, install the standard Tauri prerequisites, including `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libsoup-3.0-dev`, and `libjavascriptcoregtk-4.1-dev` on Debian/Ubuntu. The setup helpers below install the native dependencies and packaging tools for their distributions.

On Windows, the packaging script uses `stable-x86_64-pc-windows-msvc`. The installer bootstraps the [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) runtime when it is missing.

## Run locally

From the repository root:

```bash
npm install
npm run tauri dev
```

## Isolated development profile

`npm run tauri dev` and `npm run tauri:stable` launch **vorkspace Dev** with the `com.vrnrn.vorkspace.development` application profile and a frontend on port 1430. Startup prints the selected data directory. Session history, unsent drafts, Notes, app-managed provider accounts and preferences stay separate from the installed app. No production data is copied automatically. External provider CLI sign-ins remain owned by those CLIs.

Use the npm launcher for development; it applies [tauri.development.conf.json](../src-tauri/tauri.development.conf.json) before any optional config overrides. Debug binaries also enforce the development app identity before initializing native storage. Secondary windows use the same development application profile. The separate frontend origin avoids reusing browser storage from the older port-1420 development setup.

Production builds keep `com.monocode.desktop`, the existing profile, and the signed updater. `npm run tauri build` passes build arguments through without selecting the development configuration. Do not copy a production profile into the development profile for routine testing.

## Apple Silicon releases

This fork's Mac builds check [vorkspace releases](https://github.com/vrnrn/vorkspace/releases) for updates. The public verification key and feed URL live in [tauri.macos.conf.json](../src-tauri/tauri.macos.conf.json), which Tauri loads automatically on macOS. Install a build with this configuration once; existing builds cannot change their embedded updater configuration automatically.

The app uses ad hoc Apple signing and does not require an Apple Developer certificate. It is not notarized, so a downloaded build may need **System Settings → Privacy & Security → Open Anyway** on first launch. The separate updater signature still verifies every update before installation.

To build an Apple Silicon app locally, install the Rust target and supply the matching updater private key from outside the repository:

```bash
rustup target add aarch64-apple-darwin
TAURI_SIGNING_PRIVATE_KEY="$HOME/.config/vorkspace/updater/signing.key" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD='' \
npx tauri build --target aarch64-apple-darwin --bundles app,dmg
```

The app is named `vorkspace.app`, and the DMG is named `vorkspace_X.Y.Z_aarch64.dmg`. The app, updater archive, and signature are under `target/aarch64-apple-darwin/release/bundle/macos/`; the DMG is under `bundle/dmg/`. Install the app in `~/Applications` so the in-app updater can replace it without administrator access. Keep the private key backed up securely: future updates must use the same key. Never commit it.

The [Mac release workflow](../.github/workflows/macos-release.yml) builds the Apple Silicon desktop app and remote-host packages for Apple Silicon and Intel Macs. It does not build or upload Linux or Windows packages. Release titles use `vorkspace X.Y.Z`, without a platform suffix. The workflow requires the matching private key in the encrypted repository secret `MACOS_UPDATER_SIGNING_KEY`.

Run **Actions → Mac release → Run workflow** from `main`. Each new run selects a stable version newer than existing Mac release tags and drafts, using the checked-in version as a minimum. For example, if `0.13.8` is already published, the next run builds `0.13.9`. Versions are stamped consistently into the disposable CI checkout; no version-bump commit is pushed. To start a new minor or major series, use `npm run set-version -- X.Y.Z` before merging.

The native macOS build number (`CFBundleVersion`) uses `workflow-run-number.attempt`, such as `42.1`, and appears in the release body and Actions summary. Retrying a run can finish its own draft with a new attempt number; another run reserves a newer version. If a newer version has already been published, start a new workflow run instead of retrying an older draft. Explicit `mac-vX.Y.Z` tag builds still require the checked-in version to match and cannot overwrite a published release.

The workflow validates the app version, build number, arm64 architecture, updater feed, title, and Mac-only asset list before publication. Ordinary pushes to `main` do not publish a release.

The Mac release workflow is this fork’s only release channel. It retains the existing updater key and publishes the signed feed in `vrnrn/vorkspace`; no Cloudflare upload or Apple notarization credentials are needed for this flow. Automated native builds run on macOS. Linux and Windows source-build helpers remain available for manual use.

## Ubuntu / Debian packages

On an Ubuntu/Debian workstation, the repository can install the native Tauri prerequisites and build distributable Linux packages directly:

```bash
npm run setup:linux:deb
npm ci
npm run build:linux
```

The Linux build emits `.deb` and AppImage bundles under `target/release/bundle/`, in the `deb/` and `appimage/` directories. `build:linux` repacks the AppImage so it uses the host WebKitGTK 4.1 stack instead of bundled Ubuntu libraries. See [Linux installation](install.md) for runtime dependencies and package instructions.

## Fedora / Enterprise Linux packages

On Fedora or Enterprise Linux 10 (registered RHEL, Rocky, Alma, CentOS Stream, Oracle):

```bash
npm run setup:linux:fedora
npm ci
npm run build:fedora
```

On EL 10, the setup helper enables EPEL 10 and CRB automatically, since the development packages need CRB. On Oracle Linux it enables `ol10_codeready_builder` and `ol10_developer_EPEL`. Fedora does not need these extra repositories.

The build emits a `.rpm` under `target/release/bundle/rpm/`, installable with:

```bash
sudo dnf install ./target/release/bundle/rpm/vorkspace-*.rpm
```

EL 9 and older are unsupported (`webkit2gtk4.1-devel` only exists in EPEL 10). Build the `.rpm` on Enterprise Linux 10 for Fedora and EL 10 compatibility. See [Linux installation](install.md#fedora--enterprise-linux-packages) for locally built package installation and runtime repository setup.

## Windows packages

```bash
npm ci
npm run build:windows
```

The Windows build emits an NSIS installer under `target/release/bundle/nsis/`.

## Tauri configuration

- [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json) contains the shared application and bundle configuration.
- [src-tauri/tauri.macos.conf.json](../src-tauri/tauri.macos.conf.json) configures this fork's Mac updater.
- [src-tauri/tauri.linux.conf.json](../src-tauri/tauri.linux.conf.json) loads automatically for Linux development and builds.
- [src-tauri/tauri.windows.conf.json](../src-tauri/tauri.windows.conf.json) loads automatically for Windows development and builds.

## Updates

Apple Silicon releases use the signed GitHub updater feed described above. Linux and Windows source builds are not part of this fork’s published release channel; build and install a newer package to update them.

Read [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines and checks. The experimental remote host has its own [build and packaging instructions](remote-access.md#release-packaging).
