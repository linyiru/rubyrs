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
    print!("{}", rubyrs_spec_extract::extract(&source));
    ExitCode::SUCCESS
}
