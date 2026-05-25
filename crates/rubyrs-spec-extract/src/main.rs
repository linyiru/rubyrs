//! Thin CLI for `rubyrs_spec_extract::extract`.
//!
//! ```bash
//! cargo run --release -p rubyrs-spec-extract -- path/to/upstream_spec.rb
//! ```
//!
//! Output goes to stdout; redirect into the matching file under
//! `crates/rubyrs/spec/ruby/` to land it as a new spec.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: rubyrs-spec-extract <path/to/spec.rb>");
        return ExitCode::from(2);
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rubyrs-spec-extract: cannot read {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let errors = rubyrs_spec_extract::parse_errors(&source);
    if !errors.is_empty() {
        eprintln!(
            "rubyrs-spec-extract: {}: {} prism parse error(s); output is best-effort:",
            path,
            errors.len()
        );
        for e in &errors {
            eprintln!("  - {e}");
        }
    }
    print!("{}", rubyrs_spec_extract::extract(&source));
    if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        // Non-zero so a script driving the extractor over a
        // batch can flag files that need human attention,
        // without losing the partial rewrite already on stdout.
        ExitCode::from(3)
    }
}
