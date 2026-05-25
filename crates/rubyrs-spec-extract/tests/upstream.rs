//! End-to-end tests against vendored upstream ruby/spec files.
//!
//! The mini-fixtures in `tests/golden/` are hand-built to
//! exercise specific code paths in isolation. The fixtures here
//! are unmodified upstream snapshots
//! (`vendor`-style — see `tests/upstream/README.md`) — what the
//! extractor actually has to handle in the field.
//!
//! Each `<name>.rb` is paired with `<name>.expected.rb`. The
//! `.rb` file is the upstream source exactly as committed (do
//! not edit). The `.expected.rb` is what the extractor produced;
//! regenerate with:
//!
//! ```bash
//! UPDATE_EXPECTED=1 cargo test -p rubyrs-spec-extract --test upstream
//! ```
//!
//! What these tests prove and DON'T prove:
//!
//! - Prove: extractor output is stable across runs and changes;
//!   a v0.x release that regresses behaviour on real upstream
//!   files trips the assertion.
//! - Prove: extracted output parses as valid Ruby (`syntax_ok`).
//! - Don't prove: extracted output PASSES the micro-runner —
//!   many upstream files mix matchers v0.1 doesn't rewrite
//!   (`should_not`, `.should.predicate?`, `should.raise`,
//!   `it_behaves_like`), and the extracted output still
//!   contains those unmodified. Running them through the runner
//!   would raise NoMethodError on first hit. That's expected:
//!   v0.1 is a STARTER, not a complete translator. The remaining
//!   patterns ship in v0.2.

use std::path::PathBuf;

#[test]
fn upstream_string_reverse() {
    run_upstream("string_reverse_spec");
}

#[test]
fn upstream_string_empty() {
    run_upstream("string_empty_spec");
}

#[test]
fn upstream_string_length() {
    run_upstream("string_length_spec");
}

#[test]
fn upstream_outputs_parse_as_valid_ruby() {
    // Even when extraction leaves untranslated patterns in
    // place, the output should still be syntactically valid
    // Ruby. A regression that emitted a malformed `assert_eq(`
    // missing a closing paren would surface here without
    // depending on the golden text.
    let base = upstream_dir();
    for name in ["string_reverse_spec", "string_empty_spec", "string_length_spec"] {
        let path = base.join(format!("{name}.rb"));
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let output = rubyrs_spec_extract::extract(&source);
        let parsed = ruby_prism::parse(output.as_bytes());
        let errors: Vec<_> = parsed.errors().collect();
        assert!(
            errors.is_empty(),
            "extractor output for {name} did not parse cleanly: {} error(s)",
            errors.len()
        );
    }
}

fn run_upstream(name: &str) {
    let base = upstream_dir();
    let input_path = base.join(format!("{name}.rb"));
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
        "\n--- upstream golden mismatch for {name} ---\n\
         re-run with UPDATE_EXPECTED=1 to refresh if the change was intentional."
    );
}

fn upstream_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/upstream")
}
