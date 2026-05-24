//! rubund — a Rust implementation of Bundler (placeholder).
//!
//! Today this binary is intentionally a near-empty shell. It exists
//! to lock in the workspace wiring — that `rubund` builds, that it
//! links against `rubyrs` (the embedded interpreter), and that a
//! single `cargo run -p rubund` works without any version dance.
//!
//! The `--demo` flag drives one trivial `rubyrs::Runtime::eval` so
//! the rubyrs dependency isn't dead weight. Real Bundler features
//! (`Gemfile` evaluation, lockfile resolution, fetch, install) land
//! in subsequent commits.

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("rubund {VERSION} — Rust implementation of Bundler (placeholder shell)");
    println!();
    println!("USAGE:");
    println!("    rubund [--version | --help | --demo]");
    println!();
    println!("This binary is a placeholder. It currently understands:");
    println!("    --version    Print version and exit");
    println!("    --help       Print this help and exit");
    println!("    --demo       Evaluate a one-liner via the embedded rubyrs runtime");
    println!();
    println!("None of the actual Bundler commands (install, update, exec, lock)");
    println!("are implemented yet.");
}

fn run_demo() -> ExitCode {
    // Smallest possible exercise of the rubyrs embedding API — confirms
    // the workspace dep is wired up end-to-end.
    let mut rt = rubyrs::Runtime::new();
    match rt.eval(
        r#"puts "rubund #{1 + 2 + 3} — the interpreter is wired up.""#,
        "<rubund --demo>",
    ) {
        Ok(_) => ExitCode::SUCCESS,
        Err(trap) => {
            eprintln!("{}", rt.format_trap(&trap));
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("rubund {VERSION}");
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--demo") => run_demo(),
        Some(other) => {
            eprintln!("rubund: unknown argument: {other}");
            eprintln!("Try `rubund --help`.");
            ExitCode::FAILURE
        }
    }
}
