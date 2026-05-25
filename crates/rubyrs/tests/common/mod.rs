//! Shared helpers for integration tests.
//!
//! Each file under `tests/` compiles as its own crate, so anything
//! duplicated across them lives here. Pull in with `mod common;`.

#![allow(dead_code)]

/// Filename extension for Ruby C extensions on the host platform.
///
/// Matches Ruby's `RbConfig::CONFIG['DLEXT']` convention rather than
/// the platform's native dylib suffix — on macOS that means `bundle`,
/// not `dylib`, which is why `std::env::consts::DLL_SUFFIX` is wrong
/// here.
pub const DYLIB_EXT: &str = std::cfg_select! {
    target_os = "macos" => "bundle",
    windows => "dll",
    _ => "so",
};
