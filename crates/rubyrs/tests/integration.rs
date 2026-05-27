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
#[test] fn builtin_shadow() { run_fixture("builtin_shadow"); }
#[test] fn class() { run_fixture("class"); }
#[test] fn array_hash() { run_fixture("array_hash"); }
#[test] fn block() { run_fixture("block"); }
#[test] fn exception() { run_fixture("exception"); }
#[test] fn symbol_interp() { run_fixture("symbol_interp"); }
#[test] fn gc_block() { run_fixture("gc_block"); }

/// Malformed `RUBYRS_*` env-var values must trigger a stderr
/// warning AND still let the script run with the default cap
/// applied. Previously the parse failure was swallowed silently
/// (`.and_then(|s| s.parse().ok())`), so a typo like
/// `RUBYRS_MAX_FRAMES=oops` ran with the default cap and gave no
/// hint that the env var had been ignored.
#[test]
fn env_cap_typo_warns_on_stderr() {
    use std::fs;
    let tmp = manifest_dir().join("target/_env_cap_test.rb");
    fs::create_dir_all(tmp.parent().unwrap()).unwrap();
    fs::write(&tmp, "puts \"ok\"\n").unwrap();

    let out = Command::new(rubyrs_bin())
        .env("RUBYRS_MAX_FRAMES", "oops")
        .arg(&tmp)
        .output()
        .expect("failed to execute rubyrs");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "rubyrs exited non-zero; stderr:\n{}", stderr);
    assert_eq!(stdout, "ok\n", "script body still ran with default cap");
    assert!(
        stderr.contains("RUBYRS_MAX_FRAMES")
            && stderr.contains("oops")
            && stderr.contains("warning"),
        "expected typo warning in stderr; got:\n{}",
        stderr
    );

    // A correctly-formed value must NOT warn.
    let out_good = Command::new(rubyrs_bin())
        .env("RUBYRS_MAX_FRAMES", "256")
        .arg(&tmp)
        .output()
        .expect("failed to execute rubyrs");
    let stderr_good = String::from_utf8_lossy(&out_good.stderr);
    assert!(
        !stderr_good.contains("warning"),
        "well-formed value should not warn; got stderr:\n{}",
        stderr_good
    );
}

#[test] fn err_nomethod() { run_error_fixture("nomethod"); }
#[test] fn err_wrong_args() { run_error_fixture("wrong_args"); }
#[test] fn err_yield_no_block() { run_error_fixture("yield_no_block"); }

// Divergence ratchets — fixtures that pin rubyrs's CURRENT divergent
// behavior against CRuby. Each fixture's body documents the gap
// inline + the spec block it un-locks once the underlying behavior
// is brought into line; the `.expected` golden matches rubyrs today.
// When a future PR fixes the gap, the golden mismatch fires + the
// fix PR deletes the ratchet fixture and un-skips the matching
// `# skipped (divergent):` trace in `spec/ruby/*.rb`. Surfaced from
// the spec-ingestion arc in PR #167 / #158 / #188.
#[test] fn divergence_array_first_bignum() { run_fixture("divergence_array_first_bignum"); }
// `divergence_string_strip_nul` removed when this PR fixed the
// gap (vm/string.rs now strips NUL bytes alongside CRuby's
// whitespace set). Spec blocks un-skipped in string_strip_spec.rb /
// string_lstrip_spec.rb / string_rstrip_spec.rb.
#[test] fn divergence_hash_eql_keys() { run_fixture("divergence_hash_eql_keys"); }
#[test] fn divergence_hash_fetch_arity() { run_fixture("divergence_hash_fetch_arity"); }
// `break`/`next` through an `ensure` body inside a `while` loop is
// implemented with full Ruby semantics (run the ensure body, then
// complete the structured transfer). The defensive `NotImplementedError`
// trap that previously gated this case has been removed; positive
// coverage lives in `tests/diff/break_next_ensure.rb` (diff_cruby
// against CRuby as oracle).
