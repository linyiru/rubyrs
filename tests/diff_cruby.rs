//! Differential testing against CRuby.
//!
//! Each `tests/diff/*.rb` is executed under both rubyrs and the system
//! `ruby` interpreter; stdout must match byte-for-byte. CRuby acts as
//! the oracle: any deviation is a rubyrs bug (or, rarely, an
//! intentionally documented divergence — see SUBSET.md).
//!
//! If `ruby` is not on PATH, tests skip with a warning rather than fail,
//! so `cargo test` works on machines without CRuby. CI is expected to
//! provide Ruby; both ubuntu-latest and macos-latest images ship with
//! it pre-installed.

use std::path::PathBuf;
use std::process::Command;

fn rubyrs_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rubyrs"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ruby_available() -> bool {
    Command::new("ruby")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_diff(name: &str) {
    if !ruby_available() {
        eprintln!("skipping diff_cruby::{} — `ruby` not on PATH", name);
        return;
    }
    let dir = manifest_dir().join("tests/diff");
    let rb_rel = PathBuf::from("tests/diff").join(format!("{name}.rb"));
    let rb_abs = dir.join(format!("{name}.rb"));
    assert!(rb_abs.exists(), "missing diff fixture: {}", rb_abs.display());

    let ours = Command::new(rubyrs_bin())
        .current_dir(manifest_dir())
        .arg(&rb_rel)
        .output()
        .expect("failed to spawn rubyrs");
    let theirs = Command::new("ruby")
        .arg("--disable=gems")
        .current_dir(manifest_dir())
        .arg(&rb_rel)
        .output()
        .expect("failed to spawn ruby");

    assert!(
        theirs.status.success(),
        "CRuby itself failed on {} (probably a fixture bug):\n{}",
        name,
        String::from_utf8_lossy(&theirs.stderr)
    );
    assert!(
        ours.status.success(),
        "rubyrs failed on {} but CRuby succeeded:\nstderr:\n{}",
        name,
        String::from_utf8_lossy(&ours.stderr)
    );

    let ours_stdout = String::from_utf8_lossy(&ours.stdout);
    let theirs_stdout = String::from_utf8_lossy(&theirs.stdout);
    assert_eq!(
        ours_stdout, theirs_stdout,
        "stdout mismatch for {}:\n--- rubyrs:\n{}\n--- CRuby:\n{}",
        name, ours_stdout, theirs_stdout,
    );
}

#[test] fn integer_basics() { run_diff("integer_basics"); }
#[test] fn string_basics() { run_diff("string_basics"); }
#[test] fn array_basics() { run_diff("array_basics"); }
#[test] fn hash_basics() { run_diff("hash_basics"); }
#[test] fn block_basics() { run_diff("block_basics"); }
#[test] fn class_basics() { run_diff("class_basics"); }
#[test] fn symbol_basics() { run_diff("symbol_basics"); }
#[test] fn interpolation() { run_diff("interpolation"); }
#[test] fn rescue_basics() { run_diff("rescue_basics"); }
#[test] fn fizzbuzz_15() { run_diff("fizzbuzz_15"); }
#[test] fn inheritance() { run_diff("inheritance"); }
