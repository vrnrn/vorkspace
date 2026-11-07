use sha2::{Digest, Sha256};
use std::{env, fs, path::Path, process::Command};

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|error| {
        panic!("Could not run {command:?}: {error}");
    });
    assert!(status.success(), "Command failed: {command:?}");
}

fn unpack(vendor: &Path, archive: &str, checksum: &str, destination: &Path, directory: &str) {
    let source = vendor.join(archive);
    println!("cargo:rerun-if-changed={}", source.display());
    let digest = format!(
        "{:x}",
        Sha256::digest(fs::read(&source).expect("read vendored Ghostty source"))
    );
    assert_eq!(
        digest, checksum,
        "Vendored Ghostty archive changed: {archive}"
    );
    let marker = destination.join(format!("{archive}.sha256"));
    if fs::read_to_string(&marker).ok().as_deref() == Some(checksum) {
        return;
    }
    let extracted = destination.join(directory);
    if extracted.exists() {
        fs::remove_dir_all(&extracted).expect("remove outdated Ghostty extraction");
    }
    run(Command::new("tar")
        .arg("-xzf")
        .arg(source)
        .arg("-C")
        .arg(destination));
    fs::write(marker, checksum).expect("publish completed Ghostty extraction");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/bridge.c");
    println!("cargo:rerun-if-changed=native/bridge.h");
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let vendor = root.join("../../vendor/libghostty-vt");
    let out = env::var_os("OUT_DIR").unwrap();
    let out = Path::new(&out);
    let source = out.join("libghostty-vt-0.1.0-dev");
    let prefix = out.join("ghostty-install");
    let zig = env::var_os("ZIG").unwrap_or_else(|| "zig".into());
    let version = Command::new(&zig).arg("version").output().expect(
        "Building the terminal requires Zig 0.17.x. Install Zig or set ZIG to its executable path.",
    );
    assert!(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout)
                .trim()
                .starts_with("0.17."),
        "libghostty-vt is pinned to Zig 0.17.x; found {}",
        String::from_utf8_lossy(&version.stdout)
    );

    // Cargo gives each target/profile its own OUT_DIR. Extraction and Zig's
    // compiled cache stay there; builds never download or mutate vendor/.
    unpack(
        &vendor,
        "source.tar.gz",
        "3df903c36b51b95beae2a011b25c1608bcca5f7919743b649879525d34bc57d9",
        out,
        "libghostty-vt-0.1.0-dev",
    );
    unpack(
        &vendor,
        "dependencies.tar.gz",
        "b37e6e4a10c41ace2bcc6043f3462816d38f25e59ff483644a47c54bc3b08fe2",
        out,
        "zig-pkg",
    );
    let target = env::var("TARGET").unwrap();
    let zig_target = match target.as_str() {
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => {
            let arch = target.split('-').next().unwrap();
            let minimum = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "26.0".into());
            format!("{arch}-macos.{minimum}")
        }
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu".into(),
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu".into(),
        _ => panic!("libghostty-vt target is not configured: {target}"),
    };
    run(Command::new(zig)
        .current_dir(&source)
        .env("ZIG_GLOBAL_CACHE_DIR", out.join("zig-global-cache"))
        .env("ZIG_LOCAL_CACHE_DIR", out.join("zig-local-cache"))
        .arg("build")
        .arg("--system")
        .arg(out.join("zig-pkg"))
        .arg("--prefix")
        .arg(&prefix)
        .arg(format!("-Dtarget={zig_target}"))
        .args([
            "-Dcpu=baseline",
            "-Demit-lib-vt=true",
            "-Demit-xcframework=false",
            "-Dversion-string=1.4.1-dev",
            "-Dlib-version-string=0.1.0-dev+e95bed6",
            "-Doptimize=ReleaseFast",
            "-Dstrip=true",
            "-Dsimd=false",
            "-Dvt-features=-kitty-graphics,-snapshot,-search",
        ]));

    cc::Build::new()
        .file("native/bridge.c")
        .include(source.join("include"))
        .flag_if_supported("-std=c11")
        .warnings(true)
        .warnings_into_errors(true)
        .compile("vorkspace_ghostty");
    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=ghostty-vt");
}
