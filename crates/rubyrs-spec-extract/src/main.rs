//! Thin CLI for `rubyrs_spec_extract::extract`.
//!
//! ```bash
//! cargo run --release -p rubyrs-spec-extract -- path/to/upstream_spec.rb
//! ```
//!
//! For shared-example inlining (v0.4), pass each `shared/*.rb`
//! file with a repeatable `--shared` flag:
//!
//! ```bash
//! cargo run --release -p rubyrs-spec-extract -- \
//!   path/to/upstream_spec.rb \
//!   --shared path/to/shared/length.rb \
//!   --shared path/to/shared/empty.rb
//! ```
//!
//! Output goes to stdout; redirect into the matching file under
//! `crates/rubyrs/spec/ruby/` to land it as a new spec.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut consumer_path: Option<String> = None;
    let mut shared_paths: Vec<String> = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shared" => {
                let Some(p) = args.next() else {
                    eprintln!("rubyrs-spec-extract: --shared needs a path argument");
                    return ExitCode::from(2);
                };
                shared_paths.push(p);
            }
            _ if consumer_path.is_none() => consumer_path = Some(arg),
            _ => {
                eprintln!("rubyrs-spec-extract: unexpected positional arg: {arg}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(path) = consumer_path else {
        eprintln!("usage: rubyrs-spec-extract <path/to/spec.rb> [--shared <path>]...");
        return ExitCode::from(2);
    };

    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rubyrs-spec-extract: cannot read {path}: {e}");
            return ExitCode::from(1);
        }
    };

    // Read each shared file into an owned String — the
    // SharedSpec borrows from these so we keep them alive
    // for the duration of the extract call.
    let mut shared_sources: Vec<String> = Vec::with_capacity(shared_paths.len());
    for p in &shared_paths {
        match std::fs::read_to_string(p) {
            Ok(s) => shared_sources.push(s),
            Err(e) => {
                eprintln!("rubyrs-spec-extract: cannot read shared {p}: {e}");
                return ExitCode::from(1);
            }
        }
    }
    let shared_specs: Vec<rubyrs_spec_extract::SharedSpec<'_>> = shared_sources
        .iter()
        .map(|s| rubyrs_spec_extract::SharedSpec { source: s.as_str() })
        .collect();

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

    // Also check each shared file. A broken shared file would
    // produce a silently-incomplete registry, and the consumer's
    // `it_behaves_like` would fall through as "name not found"
    // with no signal that a parse error caused it. Surfacing the
    // errors keyed by path makes the actual failure mode visible.
    let mut any_shared_errors = false;
    for (i, src) in shared_sources.iter().enumerate() {
        let sh_errors = rubyrs_spec_extract::parse_errors(src);
        if !sh_errors.is_empty() {
            any_shared_errors = true;
            eprintln!(
                "rubyrs-spec-extract: --shared {}: {} prism parse error(s); shared body may be incomplete:",
                shared_paths[i],
                sh_errors.len()
            );
            for e in &sh_errors {
                eprintln!("  - {e}");
            }
        }
    }

    let output = if shared_specs.is_empty() {
        rubyrs_spec_extract::extract(&source)
    } else {
        rubyrs_spec_extract::extract_with_shared(&source, &shared_specs)
    };
    print!("{output}");

    if errors.is_empty() && !any_shared_errors {
        ExitCode::SUCCESS
    } else {
        // Non-zero so a script driving the extractor over a
        // batch can flag files that need human attention,
        // without losing the partial rewrite already on stdout.
        ExitCode::from(3)
    }
}
