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
