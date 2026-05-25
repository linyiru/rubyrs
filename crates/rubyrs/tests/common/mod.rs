//! Shared helpers for integration tests.
//!
//! Each file under `tests/` compiles as its own crate, so anything
//! duplicated across them lives here. Pull in with `mod common;`.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

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

/// Build a vendored cext example via its `build.sh` and return the
/// path to the produced shared library, named
/// `{bundle_basename}.{RUBY_DLEXT}` under `examples/{example_dir_name}/`.
///
/// Each test file wraps this in a thin `OnceLock` so the build runs
/// at most once per test binary (cargo invokes each integration test
/// in its own process). For examples whose bundle path is shared by
/// more than one test binary, `build.sh` adds its own `flock` +
/// atomic-tmpfile-rename serialisation so parallel `cargo test`
/// runs can't race on a half-written `.bundle`/`.so`. Currently
/// applies to `msgpack-cext` (7 consumers) and `counter-cext`
/// (2 consumers). The other examples are single-consumer today;
/// any future test that points a second binary at one of them
/// should add the same `flock` + atomic-rename pattern to that
/// `build.sh` before relying on it. Centralising the build steps
/// + assertions here means the existence checks, error messages,
/// and `RUBY_DLEXT` computation only have to be maintained in
/// one place — previously 12 sibling test files had inline copies.
///
/// Panics with a clear message at each failure point:
///   - `build.sh` not found at the expected path
///   - `bash build.sh` exits non-zero (i.e. `status.success() == false`)
///   - bundle file not produced after a successful build
pub fn build_cext_bundle(example_dir_name: &str, bundle_basename: &str) -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let example_dir = crate_dir.join("examples").join(example_dir_name);
    let build_sh = example_dir.join("build.sh");
    assert!(
        build_sh.exists(),
        "missing build.sh at {}",
        build_sh.display()
    );
    let build = Command::new("bash")
        .arg(&build_sh)
        .output()
        .expect("failed to spawn build.sh");
    assert!(
        build.status.success(),
        "{}/build.sh failed.\nstdout:\n{}\nstderr:\n{}",
        example_dir_name,
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
    let bundle = example_dir.join(format!("{}.{}", bundle_basename, RUBY_DLEXT));
    assert!(
        bundle.exists(),
        "build.sh did not produce {}",
        bundle.display()
    );
    bundle
}
