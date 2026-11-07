#!/usr/bin/env bash
# Use the same pinned Rust components and Zig compiler in CI and releases.
set -euo pipefail
cd "$(dirname "$0")/../.."
test "$(uname -s)" = Darwin
: "${RUNNER_TEMP:?This setup is for a GitHub Actions runner}"
: "${GITHUB_PATH:?Missing GitHub Actions path file}"
: "${GITHUB_ENV:?Missing GitHub Actions environment file}"

# rustup installs the toolchain and components declared in rust-toolchain.toml.
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
printf '%s\n' "${CARGO_HOME:-$HOME/.cargo}/bin" >> "$GITHUB_PATH"
rustup show active-toolchain
compiler_bin=$(dirname "$(rustup which cargo)")
export PATH="$compiler_bin:$PATH"
printf '%s\n' "$compiler_bin" >> "$GITHUB_PATH"
rustc --version --verbose
sdk=$(xcrun --sdk macosx --show-sdk-version)
test "${sdk%%.*}" -ge 26
printf 'VORKSPACE_BUILD_SDK=%s\n' "$sdk" >> "$GITHUB_ENV"

case "$(uname -m)" in
  arm64)
    arch=aarch64
    checksum=b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a
    ;;
  x86_64)
    arch=x86_64
    checksum=4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6
    ;;
  *) echo 'Unsupported macOS build architecture' >&2; exit 1 ;;
esac
directory=$(mktemp -d "$RUNNER_TEMP/vorkspace-zig.XXXXXX")
archive="$directory/zig.tar.xz"
curl --fail --location --retry 3 --silent --show-error \
  "https://ziglang.org/download/0.17.0/zig-$arch-macos-0.17.0.tar.xz" -o "$archive"
printf '%s  %s\n' "$checksum" "$archive" | shasum -a 256 --check
tar -xJf "$archive" -C "$directory"
rm "$archive"
compiler="$directory/zig-$arch-macos-0.17.0"
test "$("$compiler/zig" version)" = 0.17.0
printf '%s\n' "$compiler" >> "$GITHUB_PATH"
