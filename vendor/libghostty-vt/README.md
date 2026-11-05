# libghostty-vt source bundle

Upstream: https://github.com/ghostty-org/ghostty

Revision: `e95bed63330cc921f3cf2fe3d14e024396b09d43`

Public VT version: `0.1.0-dev+e95bed6`; upstream application version: `1.4.1-dev`.

Required compiler: Zig `0.17.x` (CI pins `0.17.0`).

`source.tar.gz` is the unmodified VT source distribution generated from that revision with `zig build dist -Demit-lib-vt=true -Dsimd=false`. `dependencies.tar.gz` contains its content-addressed `zig-pkg/` packages, fetched by that command. Both archives are verified by SHA-256 in `crates/vorkspace-terminal/build.rs`. That script supplies explicit version strings so Ghostty never infers a version from the surrounding Vorkspace Git repository.

| Archive | SHA-256 |
| --- | --- |
| `source.tar.gz` | `3df903c36b51b95beae2a011b25c1608bcca5f7919743b649879525d34bc57d9` |
| `dependencies.tar.gz` | `b37e6e4a10c41ace2bcc6043f3462816d38f25e59ff483644a47c54bc3b08fe2` |

Cargo unpacks these into its private build directory, uses Zig's `--system` offline package directory, and links only `libghostty-vt.a`. No binaries or external runtime libraries are vendored. The library is compiled for the Cargo target, with baseline CPU instructions, SIMD off, and `kitty-graphics`, `snapshot` and `search` disabled. All interaction uses public `<ghostty/vt.h>` APIs through the C bridge; internal `<ghostty.h>` is never used.

To update, check out the desired upstream revision in a separate directory, install the Zig version declared by its `build.zig.zon`, generate the VT distribution and populate its Zig packages, then replace both archives together. Record the revision and both new hashes here and in `build.rs`. Update the explicit version strings, extraction directory name, public-API bridge and source license notices as needed. Run the Rust engine and GPUI terminal tests and verify a packaged binary has no Ghostty dylib dependency.

`third-party.json` is upstream's general VT dependency inventory. It includes optional dependencies disabled in this build. The statically linked subset includes Ghostty, uucode and Unicode data. Their notices, including the UTF-8 decoder's Bjoern Hoehrmann attribution, are copied into app bundles. `UUCODE-LICENSE`, `UNICODE-LICENSE` and `BJOERN-HOEHRMANN-LICENSE` are from uucode revision `ea62149739404a73c202b48a33bf6dd2af4bd9b0`; its Zig package omits its `licenses/` directory, so those companion notices are retained separately.
