use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source = manifest.join("native/Clipboard.swift");
    let dictation = manifest.join("native/Dictation.swift");
    let target = manifest.join("native/DictationTarget.swift");
    let microphones = manifest.join("native/Microphones.swift");
    println!("cargo:rerun-if-changed={}", target.display());
    println!("cargo:rerun-if-changed={}", microphones.display());
    println!("cargo:rerun-if-changed={}", dictation.display());
    println!("cargo:rerun-if-changed={}", source.display());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let library = output.join("libVorkspaceClipboard.a");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => panic!("Unsupported macOS architecture"),
    };
    let mut compiler = Command::new("xcrun");
    compiler.arg("swiftc");
    if env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_some() {
        compiler.args(["-D", "DEBUG"]);
    }
    let status = compiler
        .args([
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
        .arg(dictation)
        .arg(target)
        .arg(microphones)
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
        println!("cargo:rustc-link-search=native={path}");
    }
    // The archive is lazy-linked: Settings uses only device enumeration;
    // capture, permissions and insertion remain exclusively in the helper.
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=VorkspaceClipboard");
}
