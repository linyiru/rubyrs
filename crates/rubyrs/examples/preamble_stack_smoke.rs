//! Issue #356 regression guard — startup must not overflow a small
//! main-thread stack in a debug build.
//!
//! Constructing a `Runtime` parses and compiles the always-on preamble
//! through the recursive AST→IR translator (`ast::tr`). In unoptimised
//! (debug) builds every `tr` frame carries the locals of *all* match
//! arms, so each level of structural nesting costs several KB of native
//! stack — the startup high-water reached ~2 MB. The default
//! *main-thread* stack is only 1 MB on Windows (vs 8 MB on Linux/macOS),
//! so a library embedder building in debug overflowed at startup, while
//! release (smaller frames) ran fine. `ast::tr` now grows the native
//! stack on demand via `stacker::maybe_grow`, bounding the high-water
//! to well under the 1 MB Windows limit.
//!
//! This mirrors the exact scenario from the issue. The Windows CI job
//! runs it in a debug build with the reporter's exact feature set
//! (`--no-default-features --features std-sink`) on the real 1 MB
//! Windows main thread; a clean exit means no overflow. Linux/macOS
//! can't catch a regression here — their 8 MB main thread hides it.
//!
//! Run: cargo run --example preamble_stack_smoke --no-default-features --features std-sink

use std::io::stdout;

use rubyrs::{Config, Runtime, Value};

fn main() {
    // `with_config` is where the preamble is parsed + compiled — the
    // deep-recursion path the guard protects. The resource caps mirror
    // the issue's reproduction verbatim.
    let mut rt = Runtime::with_config(Config {
        fuel: Some(1_000_000),
        max_heap_objects: Some(10_000),
        max_frames: Some(128),
        ..Default::default()
    });

    rt.register_fn("host_pid", |_args| Ok(Value::Int(std::process::id() as i64)));
    rt.set_stdout(Box::new(stdout()));

    rt.eval(r#"puts "pid is #{host_pid}""#, "inline").unwrap();

    // Reaching here on a 1 MB main-thread stack (Windows) is the whole
    // point: a stack overflow would have aborted the process before now.
    eprintln!("issue #356 smoke: Runtime built + eval ran without stack overflow");
}
