//! Golden test runner for `scripts/polish.py`. Walks the
//! `tests/polish_golden/` directory looking for `*.input.rb`
//! files, pipes each through the polish script via `python3`,
//! and diffs the stdout against the matching `*.expected.rb`.
//!
//! Mirrors the shape of `golden.rs` (which exercises the
//! extractor binary's golden corpus) so the polish post-processor
//! gets the same kind of regression coverage. Without these
//! tests, a future edit to `DROP_PATTERNS` or
//! `_strip_strings_and_comments` would silently change polish
//! output for everyone and only get noticed the next time
//! someone regenerated a spec by hand — possibly months later
//! and without a clean diff to bisect.

use std::path::PathBuf;
use std::process::Command;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/polish_golden")
}

fn polish_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/polish.py")
}

fn collect_inputs() -> Vec<PathBuf> {
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(golden_dir())
        .expect("polish_golden/ should exist")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".input.rb"))
        .collect();
    // Stable order so test failure messages are reproducible.
    inputs.sort();
    inputs
}

#[test]
fn polish_golden_corpus_matches_expected() {
    let inputs = collect_inputs();
    assert!(
        !inputs.is_empty(),
        "polish_golden/ must have at least one *.input.rb fixture; \
         add one (and its *.expected.rb sibling) when extending the \
         covered surface — empty corpus would let the test pass \
         vacuously"
    );

    let mut failures: Vec<String> = Vec::new();
    for input in &inputs {
        // Locate the matching expected file by swapping .input.rb
        // → .expected.rb. Missing expected fixture is a test
        // authoring error, not a polish regression; fail loud.
        let expected_path = {
            let s = input.to_string_lossy().replace(".input.rb", ".expected.rb");
            PathBuf::from(s)
        };
        let expected = std::fs::read_to_string(&expected_path).unwrap_or_else(|e| {
            panic!("expected fixture {} missing: {e}", expected_path.display())
        });
        let input_src = std::fs::read_to_string(input)
            .unwrap_or_else(|e| panic!("input fixture {} unreadable: {e}", input.display()));

        // Run `python3 scripts/polish.py` with the input on stdin
        // and capture stdout. Hard fail with an actionable
        // message when python3 is missing — the polish step is
        // documented as a pipeline component and CI without
        // python3 leaves it untested (which IS the bug
        // /code-review surfaced).
        let mut child = Command::new("python3")
            .arg(polish_script())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect(
                "python3 must be available on PATH for polish_golden tests; \
                 the polish step is a pipeline component documented in \
                 crates/rubyrs-spec-extract/README.md",
            );
        {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(input_src.as_bytes())
                .expect("write input to python3 stdin");
        }
        let out = child.wait_with_output().expect("wait_with_output");
        assert!(
            out.status.success(),
            "polish.py exited non-zero on {}: stderr={}",
            input.display(),
            String::from_utf8_lossy(&out.stderr)
        );

        let actual = String::from_utf8(out.stdout).expect("polish stdout utf8");
        if actual != expected {
            failures.push(format!(
                "--- {} ---\nexpected:\n{}\nactual:\n{}",
                input.file_name().unwrap().to_string_lossy(),
                expected,
                actual
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} polish_golden case(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
