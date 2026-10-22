# Building vorkspace

The root Cargo workspace owns the desktop and core. Use current stable Rust and Node.js 24+. macOS builds require Xcode with macOS platform components. Core builds on Linux also require D-Bus headers and pkg-config.

## Development and release

```sh
npm run dev -- --cwd /path/to/project
npm run build
```

Development uses the `native-dev` Cargo profile and isolated data. Type `r` then Enter to rebuild/restart; a failed build leaves the running app open. macOS output is `target/native-dev/vorkspace.app`. Release output is `target/release/vorkspace.app`; release builds open installed desktop data.

`CARGO_TARGET_DIR` and `CARGO_BUILD_TARGET` override output placement. `CARGO_BUILD_JOBS` controls build concurrency. Build scripts do not install or launch the release bundle.

## Generated assets and tests

```sh
npm ci
npm run assets
npm run check
```

The editor, formatter, and arcade bundles are committed under `crates/vorkspace-gpui/assets/`; the Vox audio bundle lives under `crates/vorkspace-core/assets/`. Their maintained sources live in `embedded/editor`, `embedded/arcade`, and `embedded/vox`. These files are embedded, so Cargo can compile without installing JavaScript dependencies. `npm run assets` installs the separate editor generator dependencies and rebuilds these bundles. Root npm dependencies are build/test tools only.

`npm run check` runs tooling, voice, host, formatting, Clippy, and workspace tests. `npm run test:rust` runs core tests before the serial GPUI suite. `npm run host:package` creates a self-contained remote host archive including Node.

See [verification](testing.md), [data selection](data-storage.md), and [signed update packaging](updates.md).
