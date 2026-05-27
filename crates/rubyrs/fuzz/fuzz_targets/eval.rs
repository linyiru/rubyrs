//! Full VM eval fuzz target.
//!
//! Lets the parsed program actually run for longer than the
//! parse-focused target, with every resource cap turned on so
//! the fuzzer never hangs on `while true; end`, `[].cycle.to_a`,
//! or `"a" * 10**9`. Only Rust panics — `panic!`, `unwrap`/`expect`
//! ICEs, `unreachable!`, `RefCell` borrow conflicts, integer
//! overflow under `debug-assertions` — fail the iteration. Every
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
use std::sync::OnceLock;

/// Move the fuzz process cwd into an empty tempdir once at
/// startup; mirror of the helper in `parse.rs` — see that file's
/// doc-comment for the full rationale (non-deterministic file
/// I/O via `require` bypasses fuel/deadline accounting). Each
/// fuzz target is its own libfuzzer binary so the `OnceLock` is
/// per-process, not shared between targets.
fn ensure_sandbox_cwd() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let tmp = std::env::temp_dir()
            .join(format!("rubyrs-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&tmp)
            .expect("ICE: fuzz sandbox tempdir creation failed");
        std::env::set_current_dir(&tmp)
            .expect("ICE: fuzz sandbox set_current_dir failed");
    });
}

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
        ..Default::default()
    };

    let mut rt = Runtime::with_config(cfg);
    let _ = rt.eval(source, "fuzz.rb");
});
