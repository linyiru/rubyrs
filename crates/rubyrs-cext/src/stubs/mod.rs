//! Spike L3-D: bulk no-op / minimal stubs for the rb_* surface
//! flori/json (and other non-trivial cexts) reference but rubyrs
//! doesn't yet support semantically.
//!
//! Every stub exists for one purpose: macOS's flat-namespace dlopen
//! eagerly resolves all bundle symbols at load time, so the host
//! binary must EXPORT each rb_* the bundle references — even ones
//! the cext only calls in cold paths — or dlopen fails outright
//! (see L4 verification: dlopen aborts on the first missing
//! `_rb_mKernel`).
//!
//! Stubs deliberately favor "harmless wrong" over "panicking
//! correct": most return Qnil / 0 / no-op. A few use
//! `unimplemented!()` when silent wrongness would corrupt
//! caller state (typically slot-pointer returns).
//!
//! Categorized by surface area so each module can evolve
//! independently when a specific gem needs better fidelity.

pub mod dispatch;
pub mod gc;
pub mod strings;
pub mod types;
