//! `rubyrs-gapscan` CLI.
//!
//! Subcommands and flags are intentionally bare today — formats and
//! `diff` will land in later commits.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
rubyrs-gapscan — scan a Ruby codebase for features outside the rubyrs subset

USAGE:
    rubyrs-gapscan scan <path> [--include-tests] [--top N] [--format text|json] [-o FILE]
    rubyrs-gapscan diff <before.json> <after.json> [--top N]

SCAN OPTIONS:
    --include-tests   Don't skip spec/ and test/ directories
    --top N           Show top N items per section (default 40)
    --format FORMAT   Output format: text (default) or json
    -o, --output FILE Write to FILE instead of stdout
    -h, --help        Print this help

DIFF OPTIONS:
    --top N           Show top N entries per section (default 20)

EXAMPLES:
    rubyrs-gapscan scan ~/code/jekyll/lib
    rubyrs-gapscan scan ~/code/jekyll/lib --format json -o jekyll.json
    rubyrs-gapscan diff before.json after.json
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
        "diff" => run_diff(args.collect()),
        other => {
            eprintln!("rubyrs-gapscan: unknown subcommand `{other}`\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[derive(Clone, Copy)]
enum Format {
    Text,
    Json,
}

fn parse_top(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<usize, ExitCode> {
    let v = it.next().ok_or_else(|| {
        eprintln!("{flag} requires a number");
        ExitCode::from(2)
    })?;
    v.parse().map_err(|_| {
        eprintln!("{flag}: not a number: {v}");
        ExitCode::from(2)
    })
}

fn write_output(text: &str, out: Option<&PathBuf>) -> std::io::Result<()> {
    if let Some(p) = out {
        std::fs::write(p, text)
    } else {
        use std::io::Write;
        std::io::stdout().write_all(text.as_bytes())
    }
}

fn run_scan(args: Vec<String>) -> ExitCode {
    let mut path: Option<PathBuf> = None;
    let mut opts = rubyrs_gapscan::ScanOptions::default();
    let mut top: usize = 40;
    let mut format = Format::Text;
    let mut output: Option<PathBuf> = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--include-tests" => opts.skip_tests = false,
            "--top" => match parse_top(&mut it, "--top") {
                Ok(n) => top = n,
                Err(c) => return c,
            },
            "--format" => match it.next().as_deref() {
                Some("text") => format = Format::Text,
                Some("json") => format = Format::Json,
                Some(other) => {
                    eprintln!("--format: unknown value `{other}` (text|json)");
                    return ExitCode::from(2);
                }
                None => {
                    eprintln!("--format requires a value");
                    return ExitCode::from(2);
                }
            },
            "-o" | "--output" => match it.next() {
                Some(p) => output = Some(PathBuf::from(p)),
                None => {
                    eprintln!("{a} requires a path");
                    return ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with("--") || other.starts_with('-') && other.len() > 1 => {
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
    let body = match format {
        Format::Text => rubyrs_gapscan::render_text(&report, top),
        Format::Json => {
            let mut j = rubyrs_gapscan::render_json(&report);
            j.push('\n');
            j
        }
    };
    if let Err(e) = write_output(&body, output.as_ref()) {
        eprintln!("output failed: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_diff(args: Vec<String>) -> ExitCode {
    let mut positional: Vec<PathBuf> = Vec::new();
    let mut top: usize = 20;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--top" => match parse_top(&mut it, "--top") {
                Ok(n) => top = n,
                Err(c) => return c,
            },
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with("--") => {
                eprintln!("diff: unknown flag `{other}`");
                return ExitCode::from(2);
            }
            other => positional.push(PathBuf::from(other)),
        }
    }
    if positional.len() != 2 {
        eprintln!("diff: expected exactly two JSON paths (before, after)\n");
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    let load = |p: &PathBuf| -> Result<rubyrs_gapscan::Report, String> {
        let body = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        rubyrs_gapscan::parse_json(&body).map_err(|e| format!("{}: {e}", p.display()))
    };
    let before = match load(&positional[0]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let after = match load(&positional[1]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let d = rubyrs_gapscan::diff(&before, &after);
    print!("{}", rubyrs_gapscan::render_text_diff(&d, top));
    ExitCode::SUCCESS
}
