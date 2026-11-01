#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]
// Modified for gpui-pre (snapshot of zed@93d7456): shaders compile at runtime,
// from shaders.metal prefixed with the checked-in scene.h.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/scene.h");
    println!("cargo:rerun-if-changed=src/shaders.metal");
    let header = fs::read_to_string("src/scene.h").unwrap();
    let shaders = fs::read_to_string("src/shaders.metal").unwrap();
    let output = PathBuf::from(env::var("OUT_DIR").unwrap()).join("stitched_shaders.metal");
    fs::write(output, format!("{header}\n{shaders}")).unwrap();
}
