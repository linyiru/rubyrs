//! Acceptance test for the Level 1 C-ext compat spike.
//!
//! Builds and runs `examples/bcrypt-cext/`, a CRuby-shape extension
//! that takes (password, salt) Strings and returns a 60-byte
//! bcrypt-formatted String. The crypto inside the bundle is a
//! deterministic stub — the test exercises:
//!
//!   1. arity-2 dispatch from `Vm::cext_require` into the C ext
//!   2. String args travelling Ruby → C via `RSTRING_PTR` /
//!      `RSTRING_LEN`
//!   3. String return travelling C → Ruby via `rb_str_new(ptr, len)`
//!   4. per-call `CExtState` reset (handles allocated inside call N
//!      don't leak into call N+1)
//!
//! If any wire is loose, one of the assertions below catches it.
//! See `crates/rubyrs/examples/bcrypt-cext/bcrypt_ext.c` for what
//! "stub crypto" means here and how to upgrade to the real gem.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn bcrypt_cext_round_trip() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let example_dir = crate_dir.join("examples/bcrypt-cext");
    let build_sh = example_dir.join("build.sh");
    assert!(build_sh.exists(), "missing build.sh at {}", build_sh.display());

    // 1. Build the bundle.
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

    // 2. Locate artefact, host-aware.
    let ext = if cfg!(target_os = "macos") {
        "bundle"
    } else if cfg!(windows) {
        "dll"
    } else {
        "so"
    };
    let bundle = example_dir.join(format!("bcrypt_ext.{}", ext));
    assert!(
        bundle.exists(),
        "build.sh did not produce {}",
        bundle.display()
    );

    // 3. Driver: call bcrypt_hash four times, dump each result on
    //    its own line so Rust can parse and compare. Spelled out
    //    explicitly because rubyrs doesn't have `String#!=` yet —
    //    the cross-string comparisons live on the Rust side.
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_bcrypt_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"
puts bcrypt_hash("hunter2", "saltsalt")
puts bcrypt_hash("hunter2", "saltsalt")
puts bcrypt_hash("hunter3", "saltsalt")
puts bcrypt_hash("hunter2", "saltdiff")
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");

    // 4. Run.
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
        4,
        "expected 4 output lines, got {}\nstdout:\n{}",
        lines.len(),
        stdout
    );

    // 5. Shape: each is 60 bytes, $2a$10$ prefixed.
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            line.len(),
            60,
            "line {} wrong length ({}): {:?}",
            i,
            line.len(),
            line
        );
        assert!(
            line.starts_with("$2a$10$"),
            "line {} missing bcrypt prefix: {:?}",
            i,
            line
        );
    }

    // 6. Determinism: same inputs → identical output.
    assert_eq!(
        lines[0], lines[1],
        "bcrypt_hash is non-deterministic for identical inputs:\n  {:?}\n  {:?}",
        lines[0], lines[1]
    );

    // 7. Password-sensitive: changing password changes output.
    assert_ne!(
        lines[0], lines[2],
        "bcrypt_hash is insensitive to the password arg:\n  pw=hunter2 → {:?}\n  pw=hunter3 → {:?}",
        lines[0], lines[2]
    );

    // 8. Salt-sensitive: changing salt changes output.
    assert_ne!(
        lines[0], lines[3],
        "bcrypt_hash is insensitive to the salt arg:\n  salt=saltsalt → {:?}\n  salt=saltdiff → {:?}",
        lines[0], lines[3]
    );
}
