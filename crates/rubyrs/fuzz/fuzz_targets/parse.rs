//! Parser + AST-to-IR translation fuzz target.
//!
//! Feeds arbitrary UTF-8 bytes through `Runtime::eval`. The fuel
//! / deadline caps are loose enough that the preamble (exception
//! classes, Object, Comparable, etc.) loads cleanly during
//! `with_config` and the user program gets to run for a couple
//! thousand ops — but tight enough that the corpus selection
//! pressure biases libfuzzer toward the parse + AST→IR
//! translation surface rather than long-running VM loops.
//!
//! Companion to `eval.rs`: this target has a smaller fuel budget
//! so each iteration finishes faster (more execs/s, broader
//! mutation surface around the parse path); the eval target
//! gets a larger budget for deeper VM dispatch coverage.
//!
//! Only Rust-level panics fail the fuzz iteration. `Err(Trap)` —
//! SyntaxError, NoMethodError, ResourceExhausted, anything else
//! — is by construction correct VM behaviour and is ignored.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rubyrs::{Config, Runtime};

fuzz_target!(|data: &[u8]| {
    let source = match std::str::from_utf8(data) {
        Ok(s) => s,
        // `Runtime::eval` takes `&str` (UTF-8); skip non-UTF-8
        // bytes here. (Real Ruby files may declare a non-UTF-8
        // source encoding via `# encoding: ...` magic comments,
        // but rubyrs's embed API doesn't expose that path — a
        // separate fuzz target would be needed to cover it.)
        Err(_) => return,
    };

    let cfg = Config {
        // 50k ops covers preamble load (~30k as of 2026-05) +
        // a few thousand ops of user code per iteration. Tight
        // enough that pathological `loop { }` shapes trap in
        // milliseconds; loose enough that the preamble's class
        // hierarchy bootstrap doesn't itself trip the cap.
        fuel: Some(50_000),
        max_value_bytes: Some(1 << 16),
        max_symbols: Some(1 << 14),
        max_frames: Some(64),
        max_heap_objects: Some(1024),
        deadline: Some(std::time::Duration::from_millis(500)),
        ..Default::default()
    };

    let mut rt = Runtime::with_config(cfg);
    let _ = rt.eval(source, "fuzz.rb");
});
