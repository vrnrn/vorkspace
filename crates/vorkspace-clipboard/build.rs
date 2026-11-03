use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source = manifest.join("native/Clipboard.swift");
    println!("cargo:rerun-if-changed={}", source.display());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let library = output.join("libVorkspaceClipboard.a");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => panic!("Unsupported macOS architecture"),
    };
    let status = Command::new("xcrun")
        .args([
            "swiftc",
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-O",
            "-module-name",
            "VorkspaceClipboard",
            "-target",
            &format!("{arch}-apple-macos11.0"),
        ])
        .arg(source)
        .arg("-o")
        .arg(&library)
        .status()
        .expect("Xcode Swift compiler is required for the native clipboard menu");
    assert!(status.success(), "Cannot build native clipboard menu");
    // Swift's ABI-stable runtime ships with macOS; only link stubs are needed.
    let info = Command::new("xcrun")
        .args(["swiftc", "-print-target-info"])
        .output()
        .expect("Cannot locate Swift runtime");
    assert!(info.status.success());
    let info = String::from_utf8(info.stdout).unwrap();
    let paths = info
        .split("\"runtimeLibraryPaths\": [")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    for path in paths.split('"').skip(1).step_by(2) {
        println!("cargo:rustc-link-arg-bins=-L{path}");
    }
    // Only the helper links the panel; the app shares this crate's protocol.
    println!("cargo:rustc-link-arg-bins=-L{}", output.display());
    println!("cargo:rustc-link-arg-bins=-lVorkspaceClipboard");
}
