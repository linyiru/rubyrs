//! Golden-file tests: each `<name>.input.rb` should produce
//! the matching `<name>.expected.rb` byte-for-byte.
//!
//! Regenerate expected files (e.g. after intentional behaviour
//! changes) with:
//!
//! ```bash
//! UPDATE_EXPECTED=1 cargo test -p rubyrs-spec-extract
//! ```
//!
//! Without the env var, a mismatch fails the test with a
//! diff-friendly assertion so the regression is obvious.

use std::path::PathBuf;

#[test]
fn golden_simple_eq() {
    run_golden("simple_eq");
}

#[test]
fn golden_skipped_patterns() {
    run_golden("skipped_patterns");
}

#[test]
fn golden_strip_require_relative() {
    run_golden("strip_require_relative");
}

#[test]
fn golden_should_not_eq() {
    run_golden("should_not_eq");
}

#[test]
fn golden_predicate_matchers() {
    run_golden("predicate_matchers");
}

#[test]
fn golden_lambda_raise() {
    run_golden("lambda_raise");
}

#[test]
fn golden_v0_2_guards() {
    run_golden("v0_2_guards");
}

#[test]
fn golden_before_each_lift() {
    run_golden("before_each_lift");
}

#[test]
fn golden_mock_int_substitute() {
    run_golden("mock_int_substitute");
}

#[test]
fn golden_skip_log_header() {
    run_golden("skip_log_header");
}

fn run_golden(name: &str) {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let input_path = base.join(format!("{name}.input.rb"));
    let expected_path = base.join(format!("{name}.expected.rb"));

    let input = std::fs::read_to_string(&input_path)
        .unwrap_or_else(|e| panic!("read {input_path:?}: {e}"));
    let actual = rubyrs_spec_extract::extract(&input);

    if std::env::var("UPDATE_EXPECTED").is_ok() {
        std::fs::write(&expected_path, &actual)
            .unwrap_or_else(|e| panic!("write {expected_path:?}: {e}"));
        eprintln!("regenerated {expected_path:?}");
        return;
    }

    let expected = std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("read {expected_path:?}: {e}"));
    assert_eq!(
        actual, expected,
        "\n--- golden mismatch for {name} ---\n\
         re-run with UPDATE_EXPECTED=1 to refresh if the change was intentional."
    );
}
