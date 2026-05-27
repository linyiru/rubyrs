//! Full VM eval fuzz target.
//!
//! Same iteration body as `parse.rs` (see `rubyrs_fuzz::run_with_caps`
//! and `src/lib.rs`'s module doc) but called with `Caps::loose()`:
//! 500k fuel, 128 frames, 4096 heap objs — 10× the tight budget.
//! Stresses dispatch, GC, method lookup, and the primitive method
//! registry. A new VM ICE will surface here first.
//!
//! Only Rust-level panics fail the fuzz iteration. `Err(Trap)` —
//! including overflow under the fuzz profile's `overflow-checks`
//! / `debug-assertions` (re-enabled in Cargo.toml against
//! `cargo fuzz`'s release default) — is by construction correct
//! VM behaviour and is ignored.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rubyrs_fuzz::{run_with_caps, Caps};

fuzz_target!(|data: &[u8]| {
    run_with_caps(data, Caps::loose());
});
