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

fn print_usage() {
    eprintln!("usage: rubyrs-spec-extract <path/to/spec.rb> [--shared <path>]...");
}

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
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            // Reject anything else that looks like a flag so a
            // typo (`--shraed`) or `--help` doesn't get swallowed
            // as the consumer path and turn into a confusing
            // "cannot read --help" error.
            _ if arg.starts_with('-') => {
                eprintln!("rubyrs-spec-extract: unknown flag: {arg}");
                print_usage();
                return ExitCode::from(2);
            }
            _ if consumer_path.is_none() => consumer_path = Some(arg),
            _ => {
                eprintln!("rubyrs-spec-extract: unexpected positional arg: {arg}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(path) = consumer_path else {
        print_usage();
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

    // Extract + collect duplicates in a single pass. The report
    // API hands back both so we don't parse every `--shared`
    // source twice (once to detect duplicates, once to inline).
    let (output, dups) = if shared_specs.is_empty() {
        (rubyrs_spec_extract::extract(&source), Vec::new())
    } else {
        let report = rubyrs_spec_extract::extract_with_shared_report(&source, &shared_specs);
        (report.output, report.duplicates)
    };

    // Warn (don't fail) when the same shared name was supplied
    // by more than one `--shared` file. The registry keeps the
    // first definition, so the user should know that ordering
    // decided which body got inlined.
    if !dups.is_empty() {
        eprintln!(
            "rubyrs-spec-extract: {} duplicate shared-example name(s) across --shared files; keeping first definition seen:",
            dups.len()
        );
        for name in &dups {
            eprintln!("  - {name}");
        }
    }
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
