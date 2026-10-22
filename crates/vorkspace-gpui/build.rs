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
}
