//! Acceptance test for the Level 1.5 C-ext compat spike.
//!
//! This is the "100% verification" gate the user asked for: not
//! "our stub round-trips correctly" but "bcrypt-ruby's actual
//! `bcrypt_ext.c` source — unmodified — links against vendored
//! openwall crypt_blowfish, runs under rubyrs, and produces
//! byte-identical output to the published bcrypt reference vectors".
//!
//! What this exercises:
//!
//!   1. CRuby-shape extension source (`#include <ruby.h>`) compiles
//!      against our `<ruby.h>` alias unchanged.
//!   2. `rb_define_module("BCrypt")` + `rb_define_class_under(...,
//!      "Engine", rb_cObject)` + `rb_define_singleton_method`
//!      produce a class lookup-able from Ruby as `BCrypt::Engine`.
//!   3. `BCrypt::Engine.__bc_crypt(pw, salt)` from Ruby dispatches
//!      to bc_crypt with arity 2.
//!   4. bc_crypt's `StringValueCStr` / `NIL_P` / `rb_str_new_frozen`
//!      / `RB_GC_GUARD` / `rb_str_new2` / `free` macros all work.
//!   5. The byte-pointer it hands to `crypt_ra` is NUL-terminated
//!      (the bcrypt $2a$ format includes the salt in the input
//!      `setting` arg, and crypt_ra reads up to `\0`).
//!   6. The bcrypt output bytes round-trip back through `rb_str_new2`
//!      → host translation → Ruby String → `puts`.
//!
//! Every byte of every assertion below was produced by Openwall's
//! crypt_blowfish reference implementation on a CRuby system and
//! published as canonical bcrypt test data.
//!
//! If this test goes red, the bcrypt path is genuinely broken
//! somewhere — there's no plausible failure mode here that doesn't
//! map to a real regression in the cext FFI.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

mod common;

/// Openwall bcrypt test vectors. Format: `(password, salt-with-cost-prefix, expected-hash)`.
///
/// Source: bcrypt reference test data shipped with openwall
/// crypt_blowfish. Each vector independently verifiable against
/// `openssl passwd -2a` or any conforming bcrypt implementation.
const REFERENCE_VECTORS: &[(&str, &str, &str)] = &[
    (
        "U*U",
        "$2a$05$CCCCCCCCCCCCCCCCCCCCC.",
        "$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW",
    ),
    (
        "U*U*",
        "$2a$05$CCCCCCCCCCCCCCCCCCCCC.",
        "$2a$05$CCCCCCCCCCCCCCCCCCCCC.VGOzA784oUp/Z0DY336zx7pLYAy0lwK",
    ),
    (
        "U*U*U",
        "$2a$05$XXXXXXXXXXXXXXXXXXXXXO",
        "$2a$05$XXXXXXXXXXXXXXXXXXXXXOAcXxm9kjPGEMsLznoKqmqw7tc8WCx4a",
    ),
];

#[test]
fn bcrypt_reference_vectors_round_trip() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let example_dir = crate_dir.join("examples/bcrypt-cext");
    let build_sh = example_dir.join("build.sh");
    assert!(build_sh.exists(), "missing build.sh at {}", build_sh.display());

    // 1. Build the bundle (and vendored crypt_blowfish, if not cached).
    let build = Command::new("bash")
        .arg(&build_sh)
        .output()
        .expect("failed to spawn build.sh");
    assert!(
        build.status.success(),
        "build.sh failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    // 2. Locate artefact.
    let bundle = example_dir.join(format!("bcrypt_ext.{}", common::DYLIB_EXT));
    assert!(
        bundle.exists(),
        "build.sh did not produce {}",
        bundle.display()
    );

    // 3. Driver: for each reference vector, call
    //    `BCrypt::Engine.__bc_crypt(password, salt)` and `puts` the
    //    result on its own line. Output order matches REFERENCE_VECTORS.
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_bcrypt_driver.rb");
    let mut script = format!("require \"{}\"\n", bundle_no_ext.display());
    for (pw, salt, _) in REFERENCE_VECTORS {
        script.push_str(&format!(
            "puts BCrypt::Engine.__bc_crypt({:?}, {:?})\n",
            pw, salt
        ));
    }
    fs::write(&driver, &script).expect("failed to write driver.rb");

    // 4. Run rubyrs against the driver.
    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        stdout,
        stderr,
    );

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        REFERENCE_VECTORS.len(),
        "expected {} output lines, got {}\nstdout:\n{}",
        REFERENCE_VECTORS.len(),
        lines.len(),
        stdout
    );

    // 5. The load-bearing assertion. Each output byte-identical
    //    to the published reference vector.
    for (i, ((pw, salt, expected), got)) in
        REFERENCE_VECTORS.iter().zip(lines.iter()).enumerate()
    {
        assert_eq!(
            got, expected,
            "vector #{}: bcrypt({:?}, {:?})\n  expected: {}\n  got:      {}",
            i, pw, salt, expected, got
        );
    }
}
