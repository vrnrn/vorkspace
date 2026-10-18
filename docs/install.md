# Installation

Download the Apple Silicon DMG from [vorkspace releases](https://github.com/vrnrn/vorkspace/releases/latest). Open `vorkspace_X.Y.Z_aarch64.dmg` and copy `vorkspace.app` to `~/Applications`. Install and sign in to at least one [supported provider](providers.md) before starting a session.

This release channel uses ad hoc signing and is not notarized. macOS may require **System Settings → Privacy & Security → Open Anyway** on first launch. In-app updates use a separate cryptographic signature.

## Bringing your MonoCode data

vorkspace uses the same `com.monocode.desktop` profile as MonoCode. On the same Mac, your sessions, settings, notes, attachments, and configured accounts are already available. Open one app at a time.

To back up your profile and convert existing Bots before switching, download [the migration helper](../scripts/migrate-monocode-to-vorkspace.sh) and preview its plan:

```bash
bash migrate-monocode-to-vorkspace.sh
```

Quit both apps and any local host, then create the private backup and run the one-time Bot conversion:

```bash
bash migrate-monocode-to-vorkspace.sh --apply
```

The helper preserves the originals, backs up the app profile, WebKit storage, preferences, and compatibility data, and migrates your existing Bot roster, channels, settings, chat transcripts, and `bots/` and `codex-bots/` directories when they do not yet exist. Run it with `--help` for recovery into an empty profile. Keychain credentials stay on the same Mac; external provider installations and project checkouts remain in place.

## Unsent messages and attachments

Ordinary unsent input recovers after restart, separately from the **Save draft** turns in history. If another window saved a different version, the recovery notice lets you compare both before choosing one. Temporary and Inbox Ask conversations remain memory-only. Unsent input for remote sessions is saved on this desktop.

Clipboard attachments are copied into private app-data storage. On an eligible cold start, background cleanup can remove copies older than one hour that no saved data references. Cleanup defers during concurrent activity or a failed scan; removing an attachment does not immediately delete its cached file. Attachments still referenced by saved history are retained.

## Linux source builds

This fork publishes Apple Silicon macOS builds. The Linux instructions below apply to packages you [build from source](building.md).

## Ubuntu / Debian packages

Install your built `.deb` with apt so its runtime dependencies are installed too:

```bash
sudo apt install ./vorkspace_*.deb
```

## Fedora / Enterprise Linux packages

On Fedora, or on an Enterprise Linux 10 system (registered RHEL, Rocky, Alma, CentOS Stream, Oracle), install your built `.rpm`. The `.rpm` declares its own runtime dependencies, so `dnf` pulls the WebKitGTK stack for you.

Fedora needs no extra repository step:

```bash
sudo dnf install ./vorkspace-*.rpm
```

Enterprise Linux 10 needs EPEL first, because `webkit2gtk4.1` is an EPEL package there. CRB is not needed to run vorkspace.

On Rocky, Alma, or CentOS Stream 10:

```bash
sudo dnf install -y epel-release
sudo dnf install ./vorkspace-*.rpm
```

On registered RHEL 10:

```bash
sudo dnf install -y https://dl.fedoraproject.org/pub/epel/epel-release-latest-10.noarch.rpm
sudo dnf install ./vorkspace-*.rpm
```

On Oracle Linux 10, `epel-release` does not enable `ol10_developer_EPEL`, which is the repository that provides WebKitGTK. Enable it before installing the rpm:

```bash
sudo dnf install -y oracle-epel-release-el10 dnf-plugins-core
sudo dnf config-manager --set-enabled ol10_developer_EPEL
sudo dnf install ./vorkspace-*.rpm
```

Build the `.rpm` on Enterprise Linux 10 for Fedora and EL 10 compatibility. EL 9 and older are unsupported (`webkit2gtk4.1-devel` only exists in EPEL 10).

For native build dependencies and packaging commands, see [building from source](building.md#fedora--enterprise-linux-packages). Building on EL 10 also needs CRB; the repository’s setup helper enables it and EPEL automatically.

## AppImage

The AppImage needs WebKitGTK 4.1 on the host, the same as the `.deb` and `.rpm`:

| Distribution | Host package |
| --- | --- |
| Debian / Ubuntu | `libwebkit2gtk-4.1-0` |
| Fedora / Enterprise Linux 10 | `webkit2gtk4.1` |
| Arch | `webkit2gtk-4.1` |

On Enterprise Linux 10, enable EPEL as described above before installing the host package. On Fedora, install WebKit if the launcher asks for it:

```bash
sudo dnf install webkit2gtk4.1
```

Keep the AppImage somewhere convenient, such as `~/Applications`. In that directory, make it executable and run it:

```bash
chmod +x vorkspace_*.AppImage
./vorkspace_*.AppImage
```

## Updates

On Apple Silicon macOS, check for signed updates in **Settings → General**. For Linux source builds, build a newer package and install it using the same commands above. The fork’s release channel does not publish Linux updater artifacts.

## Troubleshooting on Fedora / Wayland

The AppImage uses the host WebKitGTK 4.1 stack and native Wayland, like the `.deb` and `.rpm`. Set `GDK_BACKEND=x11` to keep the previous X11-forced behavior (for example NVIDIA plus Wayland):

```bash
GDK_BACKEND=x11 ./vorkspace_*.AppImage
```

Older AppImages that bundled Ubuntu-built libraries aborted with `Could not create default EGL display: EGL_BAD_PARAMETER`; current builds do not.

Return to the [README](../README.md) for Apple Silicon downloads.
