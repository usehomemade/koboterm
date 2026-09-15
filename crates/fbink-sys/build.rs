//! Builds FBInk's static library from `third_party/FBInk` using FBInk's own
//! Makefile, with the C compiler cargo resolved for the target (under
//! `cargo zigbuild` that is a zig wrapper). Skipped entirely on non-Linux
//! hosts, where the crate is empty.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "linux" {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../third_party/FBInk").canonicalize().expect("third_party/FBInk missing: run `git submodule update --init`");
    for f in ["fbink.c", "fbink.h", "fbink_internal.h", "fbink_device_id.c", "Makefile"] {
        println!("cargo:rerun-if-changed={}", src.join(f).display());
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let build = out.join("FBInk");
    if build.exists() {
        std::fs::remove_dir_all(&build).unwrap();
    }
    let st = Command::new("cp").arg("-R").arg(&src).arg(&build).status().unwrap();
    assert!(st.success(), "copying FBInk sources failed");

    let cc = cc::Build::new().get_compiler();
    let mut cc_cmd = cc.path().display().to_string();
    for a in cc.args() {
        cc_cmd.push(' ');
        cc_cmd.push_str(&a.to_string_lossy());
    }
    let ar = cc::Build::new().get_archiver();
    let ranlib = cc::Build::new().get_ranlib();
    let ar_cmd = ar.get_program().to_string_lossy().into_owned();
    let ranlib_cmd = ranlib.get_program().to_string_lossy().into_owned();
    println!("cargo:warning=FBInk CC={cc_cmd} AR={ar_cmd} RANLIB={ranlib_cmd}");

    // Cargo exports DEBUG=<bool> to build scripts; FBInk's Makefile treats any
    // defined DEBUG as "make a Debug build", which changes the output dir.
    let st = Command::new("make")
        .current_dir(&build)
        .env_remove("DEBUG")
        .args(["staticlib", "KOBO=true", "MINIMAL=true", "DRAW=1", "STRIP=true"])
        .arg(format!("CC={cc_cmd}"))
        .arg(format!("AR={ar_cmd}"))
        .arg(format!("RANLIB={ranlib_cmd}"))
        .status()
        .expect("make not found");
    assert!(st.success(), "FBInk build failed");

    println!("cargo:rustc-link-search=native={}", build.join("Release").display());
    println!("cargo:rustc-link-lib=static=fbink");
    println!("cargo:rustc-link-lib=m");
}
