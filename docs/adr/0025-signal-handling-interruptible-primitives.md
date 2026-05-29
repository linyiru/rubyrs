# 0025: Signal handling + interruptible Vm primitives

## Status

Proposed (2026-05-29). No code change in this ADR — design lands in
follow-up phases after acceptance.

Closes the design gap surfaced by ADR 0023 v4's "`sleep` no-args"
follow-up. The follow-up itself is too narrow a framing — it would
build half a signal infrastructure for one consumer. This ADR
proposes the full signal model so future consumers (`Kernel#sleep`
no-args, `Signal.trap`, Ctrl+C → clean shutdown, deadline-aware
blocking primitives) compose on a shared base.

## Context

### What's missing today

- `Kernel#sleep` (no args) raises `ArgumentError` — documented
  limitation in commit `6e5d50ff` (`Kernel#sleep` capability).
- `Signal.trap("INT") { ... }` is undefined.
- Ctrl+C against a long-running `rubyrs script.rb` either kills the
  process abruptly (default SIGINT) or, if `_http_server` is in use,
  the battery has its own ad-hoc signal handling for the prefork
  supervisor (`Stage 7d`) that doesn't extend to script-level use.
- `Config::deadline` cannot interrupt a `sleep(60)` mid-flight —
  the Vm only sees the deadline at op-dispatch boundaries, and
  `std::thread::sleep` blocks all 60 seconds regardless.
- Blocking primitives in `_http_server` (`accept`, hyper's
  `block_on` of the connection driver) have no path to "exit cleanly
  on SIGINT" without using tokio's `signal::ctrl_c()` — which itself
  is per-runtime, not Vm-wide.

### What CRuby does (target semantics)

- `sleep` (no args) blocks until SIGINT → raises `Interrupt`
  (= `SignalException::Interrupt`).
- `Signal.trap("INT") { ... }` installs a user handler. Signals
  arrive at SAFE POINTS — between Vm ops, not inside C code.
- The handler block runs on the main thread (CRuby single-Ractor
  semantics).
- `Thread#wakeup` / `Thread#run` is the other wake source; for
  single-threaded rubyrs (ADR 0015 Tier 2 threads gated), this
  reduces to signal-only.

### Cross-cutting concerns rubyrs imposes

- **Single-threaded at the language level** (ADR 0015): the user
  can't write `Thread.new { ... }` to wake the main thread, so the
  signal path is the ONLY interrupt source. Significantly simpler
  than CRuby's Thread+signal interplay.
- **Deterministic-by-default** (ADR 0017 Rule 1): signal handling
  is a host capability. Library / embed users opt in via Config;
  the default is "signals are not delivered to Ruby code, the
  process behaves like a normal stdlib-Rust binary."
- **Fuel + deadline** already provide one form of soft interrupt
  at op-dispatch boundaries. The signal infrastructure should share
  the same "safe point" mechanism, not invent a parallel one.
- **C-ext frames** (ADR 0013): signal arrival inside a c-ext frame
  must not deliver until cext returns. Same shape as the existing
  `cext_depth` Fiber.yield guard.
- **Fiber suspension** (ADR 0023): a suspended Fiber must not
  receive the interrupt until it resumes. Same shape as the
  Fiber-scoped state stash.

## Decision

**Adopt a Vm-wide `interrupt_pending: AtomicBool` flag + capability-
gated signal handler installation + deferred interrupt delivery at
existing safe points (fuel-decrement boundaries).**

The model in five points:

1. **Flag**. `Vm` (or `Runtime`) carries
   `interrupt_pending: Arc<AtomicBool>`. The host signal handler
   sets it; the Vm consumes it.

2. **Capability gate**. `Config::install_signal_handler: bool`
   (default `false`). When `true`, `Runtime::new()` installs a
   SIGINT handler (via `signal-hook` crate or a hand-rolled
   `sigaction`) whose ONLY job is `interrupt_pending.store(true,
   Ordering::SeqCst)` — async-signal-safe by construction (atomic
   store on a long-lived address).

3. **Safe points**. The Vm checks `interrupt_pending` at the same
   boundary as fuel decrements (in `dispatch_until`'s top-of-loop,
   already a hot path). When set: clear the flag, raise a Trap
   with `Interrupt` as the wrapped exception class — same machinery
   as `ResourceExhausted` from fuel exhaustion, just a different
   exception class.

4. **Interruptible primitives**. `Kernel#sleep`'s capability
   injection (`Config::sleep_for`) is replaced by an interruptible
   variant: `sleep_for: Arc<dyn Fn(Option<Duration>, &AtomicBool) ->
   Duration>` — the closure receives the deadline AND the interrupt
   flag, sleeps in a loop with short increments, and returns early
   when the flag is set. The CLI binary's closure does
   `std::thread::sleep` in 50ms chunks, checking the flag between.
   This bounds the SIGINT response latency to 50ms.

5. **User trap handlers** (Phase 4, deferred). `Signal.trap("INT") {
   block }` records the block in `Vm::signal_traps`. When the safe-
   point check sees `interrupt_pending`, if a trap is installed
   for SIGINT, the handler block runs INSTEAD of raising Interrupt.
   If the block returns normally, execution resumes; if the block
   raises, the trap propagates as a normal Vm exception. The
   handler runs at the next safe point — not in the signal
   handler's async-signal-safe context.

### Why this over the alternatives

**Alt B — pthread_kill-based blocking-syscall interrupt.** Zero-
latency response by interrupting the blocked `std::thread::sleep`
syscall directly. Rejected: requires unsafe Rust + POSIX-only +
panic-safety hazards inside the signal handler + a separate Windows
path. The 50ms polling latency under Alt A is acceptable for the
target workloads (CLI scripts, dev-mode `_http_server`).

**Alt C — Tokio-integrated `signal::ctrl_c()`.** Works well inside
the `_http_server` battery (which already has a tokio runtime). Does
NOT help CLI script use (`rubyrs script.rb` with no tokio). Would
bifurcate the design between tokio and non-tokio paths. Rejected as
the primary mechanism; may serve as an optional tokio bridge in
Phase 5 (sets the same `interrupt_pending` flag via tokio's signal
plumbing, share the Vm-side consumption code).

**Alt D — Document `sleep` no-args as out-of-scope forever.**
Acceptable as a stopgap (current state) but locks Ctrl+C → script-
clean-shutdown out of rubyrs entirely. Rejected as a destination.

**Alt E — Per-Runtime fork() supervisor handles signals.** Some
ad-hoc signal logic already exists in `_http_server`'s prefork code
(`Stage 7d` supervisor envelope). Reusing it requires the
supervisor pattern, which isn't applicable to single-process CLI
scripts. Rejected — orthogonal to the design.

## Implementation plan

**Phase 0 — `Interrupt` exception class (~1 commit)**:

0. Install `Interrupt < SignalException < Exception` in
   `preamble/exceptions.rb`. Currently rubyrs has `Exception`,
   `RuntimeError`, etc. but no signal hierarchy. Decoupled from
   the rest of this ADR — embedders may want `Interrupt` as a
   raisable class even without signal handling.

**Phase 1 — Flag + capability + handler (~2 commits)**:

1. `Vm::interrupt_pending: Arc<AtomicBool>` + initial value `false`.
2. `Config::install_signal_handler: bool` default `false`.
   `Runtime::new()`-side: when `true`, install a SIGINT handler
   (preferred: `signal-hook` crate via `signal_hook::flag::register`
   for atomic store; fallback hand-rolled `sigaction` on POSIX).
3. CLI binary `main.rs`: `install_signal_handler: true`.
4. Test: subprocess sends SIGINT to a `loop {}` script, observes
   `interrupt_pending` is set, script eventually exits (after
   Phase 2 safe-point integration).

**Phase 2 — Safe-point integration (~2-3 commits)**:

5. `dispatch_until`'s top-of-loop checks
   `interrupt_pending.load(Relaxed)` alongside the existing
   `method_return` / `fiber_yield_pending` checks. When set: clear,
   trap with `RubyError::Interrupt` (new variant) or via the
   existing `RubyError::Uncaught` path with class_name "Interrupt".
6. C-ext re-entrancy guard: signal not delivered when
   `cext_depth > 0` (defer until cext returns). Mirrors existing
   `Fiber.yield` cext guard.
7. Fiber re-entrancy: a suspended Fiber's `interrupt_pending` is
   NOT delivered until the Fiber resumes. The flag lives on the Vm,
   not the Fiber — by design — so the suspended Fiber's resume
   re-enters dispatch_until and sees the flag on its next op.
8. Subprocess tests: SIGINT during a CPU-bound loop → script
   raises `Interrupt`. Subprocess tests: trap from inside a method
   → backtrace points at the safe point, not the signal handler.

**Phase 3 — `Kernel#sleep` interruptible (~1 commit)**:

9. `Config::sleep_for` signature change: closure now takes
   `(Option<Duration>, &AtomicBool) -> Duration`. `None` Duration
   means sleep-forever-until-signal. CLI binary's closure does
   a polling loop with 50ms chunks. Pre-existing `sleep(secs)`
   callers get the same behavior (closure receives `Some(d)` and
   ignores the flag).
10. `Kernel#sleep` (no args): call the closure with `None`, then
    raise Interrupt if the flag is set, otherwise return seconds
    slept.
11. Update test:
    `sleep_default_raises_without_capability_injection` covers the
    no-capability case. New test: with capability + signal handler,
    SIGINT during `sleep` raises Interrupt within ~100ms.

**Phase 4 — `Signal.trap("INT") { ... }` user handlers (~3-4 commits)**:

12. `Vm::signal_traps: HashMap<&'static str, ObjId>` (block ObjId
    per signal name). `Signal.trap(name, &block)` installs.
13. Safe-point check: if `interrupt_pending` is set AND a trap is
    installed for SIGINT, invoke the trap block instead of raising
    Interrupt. Re-entrant Vm dispatch — same shape as `at_exit`
    handlers would use.
14. Trap return value semantics: nil → resume; previous trap value
    → return to default behavior (raise Interrupt next time).
15. Test: trap block runs at safe point, NOT in signal context.
    Test: trap block raising propagates as a normal exception.

**Phase 5 — Deadline / Fiber + signal interaction polish (~1-2 commits)**:

16. `Config::deadline` is checked at the same safe point. With
    `install_signal_handler: true`, deadline expiration sets the
    interrupt flag — gives the user a chance to catch the deadline
    via `rescue Interrupt`. (Currently deadline raises
    ResourceExhausted which is uncatchable. Discuss: is this a
    desirable union, or should deadline stay uncatchable?
    Probably orthogonal; gate as a separate Config knob.)
17. Tokio bridge (optional): in `_http_server` builds with both
    `_http_server` and `install_signal_handler`, register a tokio
    `signal::ctrl_c()` that ALSO sets the same flag, so Ctrl+C
    during server idle wakes the accept loop too.

**Total**: ~10-13 commits over 3-4 weeks.

## Risks + open questions

1. **Async-signal-safety in the handler.** The SIGINT handler must
   only call async-signal-safe APIs. `AtomicBool::store` is — it
   compiles to a single relaxed-or-stronger atomic instruction.
   Anything else inside the handler is forbidden:
   no `eprintln!` (locking), no Rust panics (catchable but not
   safe), no allocations. `signal-hook`'s `flag::register` is
   documented as async-signal-safe; that's the recommended path.

2. **Cross-platform**. POSIX uses `sigaction`; Windows uses
   `SetConsoleCtrlHandler` for Ctrl+C, which runs on a SEPARATE
   thread (not the signal handler model). The flag-set still works
   (AtomicBool from any thread is fine), but the install path
   bifurcates. `signal-hook` abstracts over POSIX; Windows needs
   its own `_install_signal_handler` arm. Defer Windows to a
   follow-up if needed; CLI users on Windows can document
   workarounds.

3. **Re-entrancy with c-ext frames.** Same shape as Fiber.yield's
   cext_depth guard — defer delivery until cext returns. Already
   solved pattern; no new risk.

4. **Signal arrival during `Runtime::new()` preamble eval.** The
   handler is installed BEFORE the preamble eval; if a SIGINT
   arrives during preamble compilation, the flag gets set, then the
   first user-script op consumes it. Could surprise the user
   ("why did my script raise Interrupt at start?"). Mitigation:
   document; alternatively, drain the flag right before the user
   eval starts.

5. **Fuel-decrement overhead.** Adding an atomic load per op adds
   ~1ns. Modern CPUs do this in the existing branch predictor
   bucket. Likely zero measurable impact on hot benchmarks (rubyrs
   is far from JIT-level op throughput). Verify with
   `bench-fib(30)`.

6. **`Signal.trap` and `eval`** (Phase 4 concern). User installs
   a trap from inside `eval "Signal.trap(...) { ... }"`. The block
   closes over `eval`'s binding. Standard Rust-side trap-block
   storage uses ObjId; closure captures are heap-managed; no new
   surface. Verify with a Phase 4 test.

7. **Catching Interrupt vs ResourceExhausted unification** (Phase
   5 risk #1). If we ever unify (deadline expiration also raises
   Interrupt), embedders that distinguish "user pressed Ctrl+C"
   from "execution timed out" need a way to tell them apart. Keep
   the two as separate variants; just share the safe-point check.

8. **Trap from inside a Fiber body.** If `Signal.trap("INT") {
   ... }` is installed and SIGINT arrives while a Fiber is
   suspended, what happens? Proposal: the trap runs on the main
   Fiber (whoever was resumed last); the suspended Fiber's resume
   is unaffected. Verify with a Fiber + trap test in Phase 4.

## Test strategy

Phase 0:
- `Interrupt < SignalException < Exception` class hierarchy via
  `klass.ancestors`.

Phase 1:
- Subprocess: send SIGINT to a process running
  `Runtime::new()` with `install_signal_handler: true`, observe
  `interrupt_pending` is set after the syscall returns.

Phase 2:
- Subprocess: SIGINT during a CPU-bound loop → script raises
  `Interrupt` within ~100ms.
- Subprocess: SIGINT during a c-ext frame → delivery deferred until
  cext returns.
- Subprocess: SIGINT during a Fiber-suspended state → delivered on
  next resume.

Phase 3:
- Subprocess: `sleep` no-args + SIGINT after 100ms → raises
  Interrupt; total runtime ~100ms (latency bound).
- Unit: existing `sleep(0.25)` capability test still passes (closure
  signature backward-compatible via ignored-flag path).

Phase 4:
- Subprocess: `Signal.trap("INT") { puts "got it"; exit 0 }` + SIGINT
  → script prints "got it" + clean exit.
- Subprocess: trap block raising propagates as exception.
- Subprocess: trap block running at safe point shows correct
  backtrace (not in signal context).

Phase 5:
- Subprocess: `_http_server` daemon + Ctrl+C during idle accept
  → clean shutdown via the tokio bridge.

## Alternatives considered (summary)

| Alternative | Why rejected |
|---|---|
| pthread_kill / cond_wait blocking-syscall interrupt | Unsafe Rust + POSIX-only + panic-safety + Windows gap; 50ms latency is acceptable. |
| Tokio-only `signal::ctrl_c` | Doesn't cover CLI script use; would bifurcate. Optional Phase 5 bridge. |
| Per-runtime fork supervisor | Orthogonal; doesn't help single-process scripts. |
| Document forever, never implement | Locks Ctrl+C → clean-shutdown out of rubyrs. Acceptable stopgap; not destination. |

## Revision log

- **2026-05-29 — v1.** Initial design. Phase 0–5 plan ~10-13
  commits. Recommends Alt A (polling + capability-gated flag) as
  primary mechanism. Risks #1 (async-signal-safety) and #2
  (Windows) called out as the two not-yet-resolved design points.

## Related

- [ADR 0017](0017-tier1-boundary.md) — Tier 1 capabilities; signal
  handling is a host capability that opts in.
- [ADR 0023](0023-true-async-streaming.md) v4 — surfaced the `sleep`
  no-args follow-up that this ADR generalizes.
- [ADR 0013](0013-cext-vm-aliasing.md) — VmBorrow contract; c-ext
  re-entrancy guard for signal delivery follows the same pattern.
- [ADR 0015](0015-tier2-and-cext-deferral.md) — Tier 2 thread
  deferral; signal handling stays single-Vm + main-thread by design,
  matching CRuby semantics under "single Ractor".
