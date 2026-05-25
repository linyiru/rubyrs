//! `rubyrs-gapscan` CLI.
//!
//! Subcommands and flags are intentionally bare today — formats and
//! `diff` will land in later commits.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
rubyrs-gapscan — scan a Ruby codebase for features outside the rubyrs subset

USAGE:
    rubyrs-gapscan scan <path> [--include-tests] [--top N]

OPTIONS:
    --include-tests   Don't skip spec/ and test/ directories
    --top N           Show top N missing classes (default 40)
    -h, --help        Print this help

EXAMPLES:
    rubyrs-gapscan scan ~/code/jekyll/lib
    rubyrs-gapscan scan some.rb --top 100
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    match cmd.as_str() {
        "" | "-h" | "--help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        "scan" => run_scan(args.collect()),
        other => {
            eprintln!("rubyrs-gapscan: unknown subcommand `{other}`\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run_scan(args: Vec<String>) -> ExitCode {
    let mut path: Option<PathBuf> = None;
    let mut opts = rubyrs_gapscan::ScanOptions::default();
    let mut top: usize = 40;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--include-tests" => opts.skip_tests = false,
            "--top" => {
                let v = match it.next() {
                    Some(v) => v,
                    None => {
                        eprintln!("--top requires a number");
                        return ExitCode::from(2);
                    }
                };
                match v.parse() {
                    Ok(n) => top = n,
                    Err(_) => {
                        eprintln!("--top: not a number: {v}");
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with("--") => {
                eprintln!("scan: unknown flag `{other}`");
                return ExitCode::from(2);
            }
            other => {
                if path.is_some() {
                    eprintln!("scan: unexpected extra positional `{other}`");
                    return ExitCode::from(2);
                }
                path = Some(PathBuf::from(other));
            }
        }
    }
    let Some(path) = path else {
        eprintln!("scan: missing <path>\n");
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let report = match rubyrs_gapscan::scan(&path, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("scan failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let unknown = rubyrs_gapscan::unknown_classes_in(&report);
    if !unknown.is_empty() {
        eprintln!(
            "warning: gapscan encountered Prism node class(es) not in its data file: {unknown:?}\n  \
             Likely a Prism upgrade. Refresh crates/rubyrs-gapscan/data/prism_node_classes.txt."
        );
    }
    print!("{}", rubyrs_gapscan::render_text(&report, top));
    ExitCode::SUCCESS
}
