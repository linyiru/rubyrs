//! Backend-agnostic scaffolding for the rubyrs tiered JIT.
//!
//! # What lives here vs. in `rubyrs`
//!
//! ADR 0002 chose a bytecode VM over a JIT, but left the door open:
//! *"the `Vm::step()` function is well-isolated… if a future contributor
//! wants to plug a JIT in, the boundary is clear."* This crate is the
//! backend-agnostic half of that boundary.
//!
//! It owns the pieces of a tiered JIT that have **no dependency on the VM
//! internals**:
//!
//! - [`JitConfig`] — the runtime knobs (enabled? tier-up threshold?),
//!   shaped like the VM's other `Config` resource knobs.
//! - [`TierDecision`] / [`JitConfig::decide`] — the pure hotness policy:
//!   given an invocation count and whether a proto is already resolved,
//!   should it tier up *now*?
//! - [`JitStats`] — the counters surfaced to embedders (mirrors the
//!   `ic-stats` feature's `IcStats`).
//!
//! What is **not** here: the actual closure-threading compiler. That code
//! must name the crate-private `Op` / `Vm` / `Value` types and call
//! `Vm::step`, all of which are `pub(crate)` inside `rubyrs`. So the
//! compiler lives in `rubyrs/src/jit.rs` behind the `jit` feature, and
//! *uses* the types defined here. Carving a public seam wide enough to
//! move that compiler out here too is the spike's headline finding — see
//! ADR 0030.
//!
//! Keeping this crate VM-free is what lets the policy be unit-tested
//! without standing up a whole interpreter, and is the seam a future
//! Cranelift backend would also consume.

#![cfg_attr(not(feature = "native"), forbid(unsafe_code))]

// The native (Cranelift) backend needs `unsafe` (transmute of the JIT'd
// code pointer to a callable fn), so the crate-wide unsafe forbid is
// dropped only under the `native` feature.
#[cfg(feature = "native")]
pub mod native;

/// Invocation count at which a proto tiers up from the interpreter
/// (tier 0) to the closure-threaded backend (tier 1).
///
/// 50 is a PoC placeholder: low enough that a recursive `fib(30)` tiers
/// up within microseconds of starting, high enough that a genuinely
/// cold one-shot proto (the toplevel `<main>`, a `require`d file's body,
/// a class body that runs once) never pays compilation. A production
/// design would tune this against the cold-start budget ADR 0002 cares
/// about — the whole point of the threshold is that warmup cost is only
/// paid where it amortises.
pub const DEFAULT_THRESHOLD: u32 = 50;

/// Runtime-tunable JIT knobs.
///
/// Mirrors the shape of the VM's other [`Config`]-style resource knobs
/// (`fuel`, `stress_gc`): cheap to copy, default-off. The `jit` cargo
/// feature decides whether the JIT is *compiled in*; this struct decides
/// whether it's *active*. Both axes are independent on purpose — a build
/// can ship the JIT compiled-in but dormant (`enabled: false`), then flip
/// it on per-`Runtime` (or via `RUBYRS_JIT=1` in the CLI) without a
/// rebuild.
///
/// [`Config`]: https://docs.rs/rubyrs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JitConfig {
    /// Runtime master switch. When `false`, the tier-up hook returns
    /// immediately and no proto is ever compiled — the VM stays purely
    /// interpreted even though the `jit` feature is compiled in.
    pub enabled: bool,
    /// Tier-up threshold; see [`DEFAULT_THRESHOLD`].
    pub threshold: u32,
}

impl Default for JitConfig {
    fn default() -> Self {
        Self { enabled: false, threshold: DEFAULT_THRESHOLD }
    }
}

/// Per-proto tier-up decision.
///
/// A pure function of the call count + config + current compile state, so
/// it can be unit-tested without a VM and reused by any backend (the
/// closure-threading one today, a Cranelift one tomorrow).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierDecision {
    /// Not hot enough yet (or the JIT is disabled): keep interpreting.
    StayInterpreted,
    /// The count just crossed the threshold: compile this proto now.
    CompileNow,
    /// Already compiled — or permanently declined — so there is nothing
    /// left to decide. Lets the caller skip the threshold comparison for
    /// the steady state (the common case once a hot proto has tiered up).
    AlreadyResolved,
}

impl JitConfig {
    /// Decide what to do for a proto with `count` invocations whose
    /// backend state is `resolved` (i.e. already compiled OR permanently
    /// declined by the backend).
    ///
    /// `resolved` is checked first so a hot proto that has already tiered
    /// up short-circuits without touching the threshold, and a declined
    /// proto is never re-examined.
    #[inline]
    pub fn decide(&self, count: u32, resolved: bool) -> TierDecision {
        if resolved {
            return TierDecision::AlreadyResolved;
        }
        if !self.enabled {
            return TierDecision::StayInterpreted;
        }
        if count >= self.threshold {
            TierDecision::CompileNow
        } else {
            TierDecision::StayInterpreted
        }
    }
}

/// JIT counters, surfaced to embedders via `Runtime::jit_stats()` and
/// dumped by the CLI under `RUBYRS_JIT_STATS=1`. Mirrors the `ic-stats`
/// feature's `IcStats`.
///
/// `Copy` so a snapshot can be returned by value; all fields are plain
/// monotonic counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JitStats {
    /// Protos that tiered up and produced a thunk vector.
    pub compiled: u64,
    /// Protos the backend inspected but refused (a shape it doesn't
    /// model). The closure-threading PoC never declines — its catch-all
    /// thunk delegates to `Vm::step` — but a stricter native backend
    /// would, and the counter is here for it.
    pub declined: u64,
    /// Ops executed through a compiled thunk vector (tier-1 dispatch).
    pub thunk_ops: u64,
    /// Specialized thunks that hit their slow path and fell back to
    /// `Vm::step` (e.g. an integer BinOp fast path that saw a non-Int
    /// operand). A "soft deopt": correctness is preserved, the
    /// specialization just didn't apply this time. A high ratio of
    /// `fallbacks / thunk_ops` means the specialization set is a poor fit
    /// for the workload.
    pub fallbacks: u64,
}

impl JitStats {
    #[inline]
    pub fn record_compiled(&mut self) {
        self.compiled += 1;
    }
    #[inline]
    pub fn record_declined(&mut self) {
        self.declined += 1;
    }
    #[inline]
    pub fn record_thunk_op(&mut self) {
        self.thunk_ops += 1;
    }
    #[inline]
    pub fn record_fallback(&mut self) {
        self.fallbacks += 1;
    }
    /// Share of tier-1 ops that took their specialized fast path rather
    /// than falling back to the interpreter. `1.0` means every executed
    /// thunk was fully specialized; `0.0` means every one delegated to
    /// `Vm::step`. Returns `0.0` before any tier-1 op has run.
    #[inline]
    pub fn fast_path_rate(&self) -> f64 {
        if self.thunk_ops == 0 {
            return 0.0;
        }
        let fast = self.thunk_ops.saturating_sub(self.fallbacks);
        fast as f64 / self.thunk_ops as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_never_compiles() {
        let cfg = JitConfig { enabled: false, threshold: 10 };
        assert_eq!(cfg.decide(0, false), TierDecision::StayInterpreted);
        assert_eq!(cfg.decide(1_000_000, false), TierDecision::StayInterpreted);
    }

    #[test]
    fn enabled_tiers_up_at_threshold() {
        let cfg = JitConfig { enabled: true, threshold: 50 };
        assert_eq!(cfg.decide(49, false), TierDecision::StayInterpreted);
        assert_eq!(cfg.decide(50, false), TierDecision::CompileNow);
        assert_eq!(cfg.decide(51, false), TierDecision::CompileNow);
    }

    #[test]
    fn resolved_short_circuits_regardless_of_count() {
        let cfg = JitConfig { enabled: true, threshold: 50 };
        // Already resolved wins even past the threshold and even when
        // disabled — there is genuinely nothing left to do.
        assert_eq!(cfg.decide(100, true), TierDecision::AlreadyResolved);
        let off = JitConfig { enabled: false, threshold: 50 };
        assert_eq!(off.decide(100, true), TierDecision::AlreadyResolved);
    }

    #[test]
    fn default_is_dormant() {
        let cfg = JitConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.threshold, DEFAULT_THRESHOLD);
        assert_eq!(cfg.decide(DEFAULT_THRESHOLD, false), TierDecision::StayInterpreted);
    }

    #[test]
    fn fast_path_rate_math() {
        let mut s = JitStats::default();
        assert_eq!(s.fast_path_rate(), 0.0);
        s.thunk_ops = 100;
        s.fallbacks = 25;
        assert!((s.fast_path_rate() - 0.75).abs() < 1e-9);
        s.fallbacks = 0;
        assert!((s.fast_path_rate() - 1.0).abs() < 1e-9);
    }
}
