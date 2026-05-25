use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Find the rubyrs binary built by cargo.
fn rubyrs_bin() -> PathBuf {
    let p = std::env::var("CARGO_BIN_EXE_rubyrs")
        .expect("CARGO_BIN_EXE_rubyrs not set; run via `cargo test`");
    PathBuf::from(p)
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run a `.rb` fixture that should succeed; compare its stdout to the
/// matching `.expected` golden file. Set `UPDATE_EXPECTED=1` to refresh.
fn run_fixture(name: &str) {
    let dir = manifest_dir().join("tests/fixtures");
    let rb_rel = PathBuf::from("tests/fixtures").join(format!("{name}.rb"));
    let expected_path = dir.join(format!("{name}.expected"));
    let rb_abs = dir.join(format!("{name}.rb"));
    assert!(rb_abs.exists(), "missing fixture: {}", rb_abs.display());
    assert!(expected_path.exists(), "missing expected: {}", expected_path.display());

    let out = Command::new(rubyrs_bin())
        .current_dir(manifest_dir())
        .arg(&rb_rel)
        .output()
        .expect("failed to execute rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "rubyrs failed on {}\nstderr:\n{}",
        rb_abs.display(), stderr
    );

    let expected = fs::read_to_string(&expected_path).unwrap();
    if std::env::var("UPDATE_EXPECTED").is_ok() && stdout != expected {
        fs::write(&expected_path, &stdout).unwrap();
        eprintln!("updated expected for {}", name);
        return;
    }
    assert_eq!(stdout, expected, "output mismatch for fixture {}", name);
}

/// Run an error fixture under `tests/fixtures/errors/<name>.rb`. Expects
/// rubyrs to exit with a non-zero code; compares stderr to the matching
/// `.expected_err` golden file. Set `UPDATE_EXPECTED=1` to refresh.
fn run_error_fixture(name: &str) {
    let dir = manifest_dir().join("tests/fixtures/errors");
    let rb_rel = PathBuf::from("tests/fixtures/errors").join(format!("{name}.rb"));
    let expected_path = dir.join(format!("{name}.expected_err"));
    let rb_abs = dir.join(format!("{name}.rb"));
    assert!(rb_abs.exists(), "missing fixture: {}", rb_abs.display());

    let out = Command::new(rubyrs_bin())
        .current_dir(manifest_dir())
        .arg(&rb_rel)
        .output()
        .expect("failed to execute rubyrs");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "expected error fixture {} to fail but it exited 0; stderr:\n{}",
        name, stderr
    );

    if std::env::var("UPDATE_EXPECTED").is_ok() {
        fs::write(&expected_path, &stderr).unwrap();
        eprintln!("updated expected_err for {}", name);
        return;
    }
    let expected = fs::read_to_string(&expected_path).unwrap_or_else(|_|
        panic!("missing expected_err: {}", expected_path.display()));
    assert_eq!(stderr, expected, "stderr mismatch for error fixture {}", name);
}

#[test] fn fizzbuzz() { run_fixture("fizzbuzz"); }
#[test] fn class() { run_fixture("class"); }
#[test] fn array_hash() { run_fixture("array_hash"); }
#[test] fn block() { run_fixture("block"); }
#[test] fn exception() { run_fixture("exception"); }
#[test] fn symbol_interp() { run_fixture("symbol_interp"); }
#[test] fn gc_block() { run_fixture("gc_block"); }

#[test] fn err_nomethod() { run_error_fixture("nomethod"); }
#[test] fn err_wrong_args() { run_error_fixture("wrong_args"); }
#[test] fn err_yield_no_block() { run_error_fixture("yield_no_block"); }
// Pins the defensive trap for `break` through an `ensure` body inside
// a `while` loop. Full Ruby semantics (run the ensure body, then
// exit the loop with the break value) requires a break-aware Trap
// variant + Op::Raise hook — too large to land alongside the basic
// break-in-while fix. Until that lands, we error with a clear message
// rather than silently dropping the ensure body. The .expected_err
// pins that message so a future regression that re-silences the
// case (or, conversely, the proper fix that removes the trap) shows
// up as a test diff.
#[test] fn err_break_through_ensure() { run_error_fixture("break_through_ensure"); }
