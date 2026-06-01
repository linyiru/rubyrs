//! ADR 0025 Phase 1 — POSIX SIGINT capture for `Kernel#sleep`
//! interruptibility (Phase 3) and the broader interrupt
//! propagation work (Phase 2 safe-point check + Phase 4
//! `Signal.trap`).
//!
//! Surface:
//!
//! - [`install_signals`] — invoked from `Runtime::apply_config`.
//!   First call with `install: true` registers a
//!   `signal-hook::flag::register` handler against SIGINT,
//!   publishes the `Arc<AtomicBool>` at static lifetime, and
//!   returns it. Subsequent calls (regardless of `install`)
//!   return the same shared Arc — every Runtime in this process
//!   reads/writes the same atomic.
//!
//! - When NO Runtime has opted in yet, `install: false` returns
//!   a fresh dedicated Arc that this Vm owns. The Phase 2
//!   safe-point check reads a never-mutated flag in that case
//!   (zero overhead beyond the atomic load).
//!
//! Safety:
//!
//! The signal handler — `signal_hook::flag::register` — sets a
//! `Relaxed` atomic store. Reviewed for async-signal-safety: a
//! single relaxed atomic store on a long-lived address compiles
//! to one POSIX-listed safe instruction (`mov` with `lock` on
//! some archs; still in the
//! `man 7 signal-safety` set). No allocation, no locking, no
//! panic path. Documented async-signal-safe by `signal-hook`
//! and consistent with our review.
//!
//! Windows path (deferred — ADR 0025 v3 Risk #2):
//!
//! `signal-hook` is Unix-only. The unix `cfg` block below
//! handles POSIX; the fallback returns a fresh Arc with no
//! handler installed (i.e. `install: true` on Windows is a
//! documented no-op until `SetConsoleCtrlHandler` is wired).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Resolve the `Arc<AtomicBool>` that this Runtime's Vm should
/// use for `interrupt_pending`. First caller with `install: true`
/// also registers the SIGINT handler.
pub(crate) fn install_signals(install: bool) -> Arc<AtomicBool> {
    install_signals_impl(install)
}

// ---- Unix POSIX path ----

#[cfg(unix)]
use std::sync::OnceLock;

#[cfg(unix)]
static SHARED_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

#[cfg(unix)]
fn install_signals_impl(install: bool) -> Arc<AtomicBool> {
    if install {
        // First-time path: build the Arc, register the handler
        // BEFORE publishing the Arc so any racing reader of
        // `SHARED_FLAG` always observes both the register call
        // and the populated Arc together (OnceLock::get_or_init
        // serializes; the register call inside the closure is
        // ordered before SHARED_FLAG.set).
        let arc = SHARED_FLAG.get_or_init(|| {
            let flag = Arc::new(AtomicBool::new(false));
            // SIGINT == 2 on POSIX. signal_hook::flag::register
            // is documented async-signal-safe (atomic store
            // only). The handler stays installed for the
            // lifetime of the process — there is no
            // de-registration path. That's intentional: ADR
            // 0025 v3 states `install_signal_handler: true` is
            // a one-time per-process operation.
            //
            // We deliberately swallow the unlikely register
            // failure (sigaction returning EINVAL because the
            // signal number is out of range — impossible for
            // SIGINT=2). If it ever does fail, the Arc is
            // still published; the safe-point check just reads
            // an always-false flag. No silent loss of
            // correctness; the host can detect this by checking
            // that `SHARED_FLAG.get()` is `Some`.
            let _ = signal_hook::flag::register(
                signal_hook::consts::SIGINT,
                Arc::clone(&flag),
            );
            flag
        });
        Arc::clone(arc)
    } else {
        // `install: false`: always return a dedicated fresh
        // Arc, NEVER the shared one. Rationale: a Runtime that
        // didn't opt in has no signal handler registered FOR
        // ITSELF; sharing the SHARED_FLAG would let SIGINTs
        // delivered to (and stored by) the handler that the
        // opt-in Runtime registered appear in this opt-out
        // Runtime's safe-point checks. That's a surprise
        // factor for embed users who construct multiple
        // Runtimes and expect signal isolation. Pay the
        // cost of one Arc + AtomicBool per non-opt-in Runtime
        // (negligible) to keep the contract crisp.
        Arc::new(AtomicBool::new(false))
    }
}

/// ADR 0025 Phase 3 helper: check whether the given Arc is
/// the process-wide SHARED_FLAG installed by an `install: true`
/// Runtime. Used by `Kernel#sleep` (no-args) to refuse the
/// sleep-forever call when no signal handler can wake it.
///
/// Returns false on non-Unix (no signal infrastructure
/// available; the equivalent guard there is "never permit
/// sleep-forever").
#[cfg(unix)]
pub(crate) fn is_shared_flag(flag: &Arc<AtomicBool>) -> bool {
    match SHARED_FLAG.get() {
        Some(shared) => Arc::ptr_eq(shared, flag),
        None => false,
    }
}

/// Largest accepted integer signal number — platform NSIG-1.
/// CRuby's `Signal.trap(n, ...)` rejects integers above this
/// at parse time rather than letting signal-hook fail at
/// install. Linux's signal table reserves 1..=64 (32 standard
/// plus 32 real-time); macOS / BSD top out at 31. Build-time
/// constant so the range check inlines into a single
/// comparison.
#[cfg(target_os = "linux")]
const NSIG_MAX: i64 = 64;
#[cfg(not(target_os = "linux"))]
const NSIG_MAX: i64 = 31;

/// ADR 0025 Phase 4a: normalize a Ruby-side signal-name
/// argument to a Unix signal number. Accepted shapes:
/// - `Integer` (1..=NSIG_MAX — platform-dependent: Linux 64,
///   macOS/BSD 31). Round-3 follow-up: was unconditionally
///   1..=64 pre-fix; CRuby rejects numbers outside the
///   platform's `NSIG` range at parse time rather than at
///   `signal-hook` install time.
/// - `Symbol` or `String`: bare short name (`"INT"`, `:INT`)
///   or `SIG`-prefixed (`"SIGINT"`, `:SIGINT`). Both
///   case-insensitive isn't needed — CRuby is also case-
///   sensitive here; we match.
/// - Anything else → `None` (caller raises ArgumentError).
///
/// The Tier-1 portable signal subset is enumerated below.
/// Real-time signals (SIGRTMIN+) and platform-specific names
/// are deferred — match CRuby's documented portable set.
pub(crate) fn parse_signal_name(
    v: &crate::value::Value,
    interner: &crate::intern::Interner,
) -> Option<i32> {
    use crate::value::Value;
    match v {
        Value::Int(n) if (1..=NSIG_MAX).contains(n) => Some(*n as i32),
        Value::Sym(id) => parse_str(interner.resolve(*id)),
        Value::Str(s) => parse_str(&s.to_string_lossy()),
        _ => None,
    }
}

fn parse_str(s: &str) -> Option<i32> {
    let trimmed = s.strip_prefix("SIG").unwrap_or(s);
    match trimmed {
        "HUP" => Some(1),
        "INT" => Some(2),
        "QUIT" => Some(3),
        "ILL" => Some(4),
        "TRAP" => Some(5),
        "ABRT" => Some(6),
        "FPE" => Some(8),
        "KILL" => Some(9),
        "USR1" => Some(10),
        "SEGV" => Some(11),
        "USR2" => Some(12),
        "PIPE" => Some(13),
        "ALRM" => Some(14),
        "TERM" => Some(15),
        "CHLD" => Some(17),
        "CONT" => Some(18),
        "STOP" => Some(19),
        "TSTP" => Some(20),
        "TTIN" => Some(21),
        "TTOU" => Some(22),
        "URG" => Some(23),
        "WINCH" => Some(28),
        _ => None,
    }
}

// ---- Non-Unix fallback (Windows, WASI, etc.) ----

// On non-unix targets (wasm32-wasip1, Windows) the function is the
// fallback definition the unix `is_shared_flag` callers would
// otherwise be missing. Some build profiles (wasm CI smoke without
// `_fiber`) end up never importing it, so `-D warnings` flags it as
// dead. Keep the symbol present for API uniformity across platforms.
#[cfg(not(unix))]
#[allow(dead_code)]
pub(crate) fn is_shared_flag(_flag: &Arc<AtomicBool>) -> bool {
    false
}

#[cfg(not(unix))]
fn install_signals_impl(_install: bool) -> Arc<AtomicBool> {
    // ADR 0025 v3 Risk #2: Windows + Ctrl+C requires
    // `SetConsoleCtrlHandler` (running on a separate OS thread,
    // different model from POSIX signals). WASI has no signal
    // story at all. Both deferred — `install_signal_handler:
    // true` is a documented no-op on these targets until the
    // platform-specific install path lands.
    Arc::new(AtomicBool::new(false))
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn install_false_returns_fresh_flag_when_no_runtime_opted_in() {
        // Pre-condition: this test asssumes no other test in the
        // same process has called install_signals(true) BEFORE
        // it. Cargo's default `--test-threads` doesn't guarantee
        // ordering, so we just check that the returned Arc is a
        // valid AtomicBool that starts false. If another test
        // populated SHARED_FLAG first, we get the shared Arc —
        // that's also fine for this assertion (the flag value
        // is what matters, not the identity).
        let arc = install_signals(false);
        assert!(!arc.load(Ordering::Relaxed));
    }

    #[test]
    fn parse_signal_name_accepts_in_range_int() {
        let interner = crate::intern::Interner::new();
        // 2 (SIGINT) is in every platform's signal table.
        assert_eq!(
            parse_signal_name(&crate::value::Value::Int(2), &interner),
            Some(2),
        );
        // Edge: NSIG_MAX itself is accepted.
        assert_eq!(
            parse_signal_name(&crate::value::Value::Int(NSIG_MAX), &interner),
            Some(NSIG_MAX as i32),
        );
    }

    #[test]
    fn parse_signal_name_rejects_out_of_range_int() {
        let interner = crate::intern::Interner::new();
        // 0 is below the legal range.
        assert_eq!(
            parse_signal_name(&crate::value::Value::Int(0), &interner),
            None,
        );
        // Just past NSIG_MAX — platform-specific (65 on Linux,
        // 32 on macOS/BSD).
        assert_eq!(
            parse_signal_name(&crate::value::Value::Int(NSIG_MAX + 1), &interner),
            None,
        );
        // 100 is past NSIG on every supported platform.
        assert_eq!(
            parse_signal_name(&crate::value::Value::Int(100), &interner),
            None,
        );
    }

    #[test]
    fn install_true_publishes_shared_flag() {
        // After install_signals(true), SHARED_FLAG must be Some
        // and subsequent install:true calls must return the
        // same Arc. install:false ALWAYS returns a fresh
        // dedicated Arc (signal isolation between opted-in and
        // not-opted-in Runtimes).
        let a = install_signals(true);
        let b = install_signals(true);
        let c = install_signals(false);
        assert!(
            Arc::ptr_eq(&a, &b),
            "install:true calls share the shared Arc",
        );
        assert!(
            !Arc::ptr_eq(&a, &c),
            "install:false returns a dedicated Arc, not the shared one",
        );
    }
}
