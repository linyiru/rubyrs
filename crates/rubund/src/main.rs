//! rubund — a Rust implementation of Bundler (work in progress).
//!
//! The library half of this crate (`rubund::parser`) is a real,
//! tested zero-copy `Gemfile.lock` parser. The CLI half — this
//! binary — is intentionally a near-empty shell while the surface
//! commands (`install`, `update`, `exec`, `lock`, `check`) are still
//! being designed. Until then `rubund --help` is the most useful
//! thing it does.

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("rubund {VERSION} — Rust implementation of Bundler (CLI in development)");
    println!();
    println!("USAGE:");
    println!("    rubund [--version | --help]");
    println!();
    println!("This binary is a placeholder. It currently understands:");
    println!("    --version    Print version and exit");
    println!("    --help       Print this help and exit");
    println!();
    println!("None of the actual Bundler commands (install, update, exec, lock)");
    println!("are implemented yet. The real value today is the library API —");
    println!("see `rubund::parser` and the examples/ directory.");
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
        Some(other) => {
            eprintln!("rubund: unknown argument: {other}");
            eprintln!("Try `rubund --help`.");
            ExitCode::FAILURE
        }
    }
}
