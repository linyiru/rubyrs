use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Find the rubyrs binary built by cargo.
fn rubyrs_bin() -> PathBuf {
    // CARGO_BIN_EXE_rubyrs is set by cargo for integration tests on the main bin.
    let p = std::env::var("CARGO_BIN_EXE_rubyrs")
        .expect("CARGO_BIN_EXE_rubyrs not set; run via `cargo test`");
    PathBuf::from(p)
}

fn run_fixture(name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let rb = dir.join(format!("{name}.rb"));
    let expected_path = dir.join(format!("{name}.expected"));
    assert!(rb.exists(), "missing fixture: {}", rb.display());
    assert!(expected_path.exists(), "missing expected: {}", expected_path.display());

    let out = Command::new(rubyrs_bin())
        .arg(&rb)
        .output()
        .expect("failed to execute rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "rubyrs failed on {}\nstderr:\n{}",
        rb.display(),
        stderr
    );

    let expected = fs::read_to_string(&expected_path).unwrap();

    if std::env::var("UPDATE_EXPECTED").is_ok() && stdout != expected {
        fs::write(&expected_path, &stdout).unwrap();
        eprintln!("updated expected for {}", name);
        return;
    }

    assert_eq!(stdout, expected, "output mismatch for fixture {}", name);
}

#[test] fn fizzbuzz() { run_fixture("fizzbuzz"); }
#[test] fn class() { run_fixture("class"); }
#[test] fn array_hash() { run_fixture("array_hash"); }
#[test] fn block() { run_fixture("block"); }
