//! `rubyrs-wasm-timer` — sub-millisecond wall-clock timer for child
//! processes. Drop-in replacement for `python3 -c '...time_ns()...'`
//! and `/usr/bin/time -p` inside `perf/wasm_breakdown.sh`, with two
//! properties they don't have:
//!
//! 1. **ns-precision wall measurement.** `/usr/bin/time -p` rounds
//!    to 10 ms on macOS (BSD `time(1)`'s `-p` shape); useless for
//!    the sub-10 ms cwasm cold-starts this exists to characterize.
//!
//! 2. **Negligible wrapper overhead.** A `python3 -c ...` invocation
//!    adds ~1-2 ms of interpreter startup to every measurement —
//!    a 12-25% noise floor when the thing being measured is ~8 ms.
//!    This Rust binary's own startup is ~50-200 us (one Command
//!    construction + Instant capture + fork/exec), measured by
//!    spawning `/bin/true`.
//!
//! Usage: `rubyrs-wasm-timer <prog> [args...]`
//!
//! Streams the child's stdout/stderr through unchanged (timer prints
//! a sentinel `wasm-timer\twall_us\t<N>` line on its OWN stderr after
//! the child exits — appended after any child stderr content). Exits
//! with the child's exit code, or `128 + signal` on Unix if the
//! child was killed by a signal — matches bash's `$?` convention so
//! pipelines see the same number they would without the wrapper.
//!
//! Position the `Instant::now()` AS LATE AS POSSIBLE (right before
//! `Command::status()`) so the measurement excludes our own arg
//! parsing and `Command` construction — what we report is the
//! "subprocess wall time the host actually paid", which is the
//! number `perf/wasm_breakdown.sh` then subtracts the rubyrs trace
//! deltas from.

use std::env;
use std::process::{Command, ExitCode};
use std::time::Instant;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let prog = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: rubyrs-wasm-timer <prog> [args...]");
            eprintln!();
            eprintln!("Execs <prog> with the remaining args, streams its");
            eprintln!("stdio through, and on exit prints a single line to");
            eprintln!("stderr: `wasm-timer\\twall_us\\t<microseconds>`.");
            eprintln!("Exits with the child's exit code (128+sig on signal).");
            return ExitCode::from(2);
        }
    };

    let mut cmd = Command::new(&prog);
    cmd.args(args);
    // stdin/stdout/stderr default to inheriting the parent's — no
    // explicit `stdio()` needed. Don't capture: we want the child's
    // output to flow straight through to its real consumer (the
    // breakdown script's `>/dev/null 2>file` redirects).

    // Anchor t0 RIGHT before spawn so our own setup (arg parsing,
    // Command construction) doesn't inflate the reported figure.
    let t0 = Instant::now();
    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rubyrs-wasm-timer: failed to exec {:?}: {}", prog, e);
            return ExitCode::from(2);
        }
    };
    let elapsed_us = t0.elapsed().as_micros();

    // Sentinel-prefixed so the breakdown script's parser can grep
    // unambiguously even if the child's stderr happens to contain
    // bare integer lines (wasmtime occasionally logs numeric data
    // in verbose modes).
    eprintln!("wasm-timer\twall_us\t{}", elapsed_us);

    // Propagate the child's exit shape. Unix: 128+sig when killed
    // by a signal. Other platforms: fall back to 1 (no portable
    // signal info — Windows reports the raw NTSTATUS, which we
    // simplify rather than leak through).
    let exit_code: u8 = if let Some(c) = status.code() {
        // `status.code()` is i32; truncate to u8 the same way the
        // shell does (exit codes are already conventionally 0-255).
        (c as u32 & 0xff) as u8
    } else {
        #[cfg(unix)]
        {
            (128u32.saturating_add(status.signal().unwrap_or(0) as u32) & 0xff) as u8
        }
        #[cfg(not(unix))]
        {
            1
        }
    };
    ExitCode::from(exit_code)
}
