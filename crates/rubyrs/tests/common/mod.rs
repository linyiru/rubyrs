//! Shared helpers for integration tests.
//!
//! Each file under `tests/` compiles as its own crate, so anything
//! duplicated across them lives here. Pull in with `mod common;`.

#![allow(dead_code)]

/// Filename suffix (no leading dot) for Ruby C extensions on the host
/// platform — Ruby's `RbConfig::CONFIG['DLEXT']` convention.
///
/// On macOS this is `"bundle"`, not `"dylib"`, which is why neither
/// `std::env::consts::DLL_SUFFIX` (returns `".dylib"`) nor
/// `libloading::library_filename` is a usable substitute here.
pub const RUBY_DLEXT: &str = std::cfg_select! {
    target_os = "macos" => "bundle",
    windows => "dll",
    _ => "so",
};
