//! Parser + AST-to-IR translation fuzz target.
//!
//! Same iteration body as `eval.rs` (see `rubyrs_fuzz::run_with_caps`
//! and `src/lib.rs`'s module doc) but called with `Caps::tight()`:
//! 50k fuel, 64 frames, 1024 heap objs. Each iteration finishes
//! in milliseconds, so the corpus selection pressure biases
//! libfuzzer toward the parse + AST→IR translation surface
//! rather than long-running VM loops.
//!
//! Only Rust-level panics fail the fuzz iteration. `Err(Trap)` —
//! SyntaxError, NoMethodError, ResourceExhausted, anything else
//! — is by construction correct VM behaviour and is ignored.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rubyrs_fuzz::{run_with_caps, Caps};

fuzz_target!(|data: &[u8]| {
    run_with_caps(data, Caps::tight());
});
