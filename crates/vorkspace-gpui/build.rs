use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let lockfile = manifest
        .ancestors()
        .map(|dir| dir.join("Cargo.lock"))
        .find(|path| path.is_file())
        .expect("The native workspace requires Cargo.lock");
    println!("cargo:rerun-if-changed={}", lockfile.display());
    let lock = fs::read_to_string(&lockfile).expect("The native workspace requires Cargo.lock");
    // Cargo writes canonical TOML. Check package names rather than license metadata:
    // zed-scap advertises MIT but its published license has additional restrictions.
    assert!(
        !lock
            .lines()
            .any(|line| line.trim() == "name = \"zed-scap\""),
        "zed-scap is prohibited in the native dependency graph, including optional dependencies. \
         Restore the GPUI patches that remove it, or replace it with a dependency whose \
         license has been reviewed. Do not suppress this check based on its MIT metadata."
    );
    build_clipboard(&manifest);
    let notes = lockfile.parent().unwrap().join("build/release-notes.md");
    println!("cargo:rerun-if-changed={}", notes.display());
    println!("cargo:rerun-if-env-changed=VORKSPACE_RELEASE_BUILD");
    let contents = fs::read_to_string(&notes).unwrap_or_default();
    let heading = format!("## [{}]", env::var("CARGO_PKG_VERSION").unwrap());
    let matching =
        contents.lines().next() == Some(heading.as_str()) && contents.len() <= 1024 * 1024;
    if env::var("VORKSPACE_RELEASE_BUILD").as_deref() == Ok("1") {
        assert!(
            matching,
            "Release builds require version-matched generated release notes"
        );
    }
    let destination = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("release-notes.md");
    fs::write(destination, if matching { contents.as_str() } else { "" })
        .expect("Cannot embed release notes");
}

fn build_clipboard(manifest: &std::path::Path) {
    use std::process::Command;
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
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
        println!("cargo:rustc-link-search=native={path}");
    }
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=VorkspaceClipboard");
}
