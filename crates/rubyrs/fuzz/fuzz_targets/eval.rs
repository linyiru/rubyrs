//! Full VM eval fuzz target.
//!
//! Lets the parsed program actually run for longer than the
//! parse-focused target, with every resource cap turned on so
//! the fuzzer never hangs on `while true; end`, `[].cycle.to_a`,
//! or `"a" * 10**9`. Only Rust panics — `panic!`, `unwrap`/`expect`
//! ICEs, `unreachable!`, `RefCell` borrow conflicts, integer
//! overflow under the fuzz profile's `overflow-checks`/
//! `debug-assertions` (re-enabled in Cargo.toml against `cargo
//! fuzz`'s release default) — fail the iteration. Every
//! other outcome is a script-level error (`Trap`) and is by
//! definition correct VM behaviour.
//!
//! Pairs with `parse.rs`: that one biases toward parser + AST→IR
//! coverage with a tighter budget; this one stresses dispatch,
//! GC, method lookup, and the primitive method registry. A new
//! VM ICE will surface here first.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rubyrs::{Config, Runtime};
use rubyrs_fuzz::ensure_sandbox_cwd;

fuzz_target!(|data: &[u8]| {
    ensure_sandbox_cwd();
    let source = match std::str::from_utf8(data) {
        Ok(s) => s,
        // Same UTF-8 gate as `parse.rs` — `Runtime::eval` takes
        // `&str`. See that file's note on `# encoding:` magic
        // comments for the deeper context.
        Err(_) => return,
    };

    let cfg = Config {
        // Larger op budget than parse.rs — preamble + nontrivial
        // user code (small recursion, a few iterators) should run
        // to completion. Tuned for ~2k exec/s.
        fuel: Some(500_000),
        max_value_bytes: Some(1 << 16),
        max_symbols: Some(1 << 14),
        max_frames: Some(128),
        max_heap_objects: Some(4096),
        deadline: Some(std::time::Duration::from_millis(500)),
        // See parse.rs comment — pin stress_gc off so the
        // harness doesn't inherit STRESS_GC=1 from the runner
        // env via `Config::default()`.
        stress_gc: false,
        ..Default::default()
    };

    let mut rt = Runtime::with_config(cfg);
    let _ = rt.eval(source, "fuzz.rb");
});
