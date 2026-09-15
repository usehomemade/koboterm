//! Raw FFI to FBInk (GPL-3.0-or-later, vendored at `third_party/FBInk`).
//! Bindings are pre-generated for `armv7-unknown-linux-musleabihf`; see
//! `build.rs` for how the C library is built. Empty on non-Linux hosts so the
//! workspace still builds and tests on a laptop.
#![no_std]

#[cfg(target_os = "linux")]
mod bindings;
#[cfg(target_os = "linux")]
pub use bindings::*;
