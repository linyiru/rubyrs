# 0025: Signal handling + interruptible Vm primitives

## Status

Proposed (2026-05-29). **v2** — three parallel reviewer rounds on
v1 surfaced six load-bearing corrections, addressed inline below.
Phase 0 has SHIPPED (commit `a5337fd7`); the rest of the design
lands in follow-up phases after acceptance.

**v1 → v2 changes**:

- **Phase 0 LANDED** (`Interrupt < SignalException < Exception`
  hierarchy in the preamble + BUILTIN_EXCEPTION_PARENT + tests).
  Commit `a5337fd7`. v1 listed it as "~1 commit" pending; v2
  marks it done.
- **`sleep(secs)` interruptibility — CRuby parity correction.** v1
  Phase 3 said `sleep(secs)` callers "get the same behavior
  (closure receives Some(d) and ignores the flag)." That diverges
  from CRuby: `sleep(10)` IS interruptible by SIGINT and returns
  elapsed integer seconds. v2 specifies that BOTH `sleep(secs)`
  and `sleep` (no args) poll the flag; the difference is only
  the upper bound (Some(d) vs None).
- **`Signal.trap` return value — CRuby parity correction.** v1
  garbled the contract. CRuby's `Signal.trap(sig, handler) →
  previous_handler` returns the PREVIOUS handler ("DEFAULT",
  "IGNORE", a Proc, or nil); the *input* is "DEFAULT" / "IGNORE"
  / a Proc / a block. v2 corrects.
- **`at_exit` × `trap("INT") { exit }`** — v1 omitted entirely.
  The canonical pattern is `trap("INT") { exit }` raising
  SystemExit, which propagates through `at_exit` handlers. v2
  names this + flags SystemExit as a Phase 4 dependency.
- **SystemExit class hierarchy.** v1 mentioned in passing only.
  CRuby places SystemExit < Exception (not under SignalException
  despite the name; SignalException is for SIG{TERM,HUP,...}
  shapes). v2 adds Phase 0.5: install `SystemExit < Exception`
  in the preamble + corresponding BUILTIN_EXCEPTION_PARENT entry.
  Decoupled from the rest like Phase 0 was.
- **Memory ordering lockdown.** v1 Phase 2 said "load(Relaxed)
  alongside fuel decrement." Relaxed-load + SeqCst-store is
  sufficient for a SINGLE flag with no paired data. v2 adds the
  explicit contract: "flag is the ONLY signal-set state; any
  future paired state (e.g. signal-name discriminant, trap
  handler ObjId) requires upgrading the load to Acquire and
  pairing the store with Release."
- **Windows path detail (Risk #2).** v1 said Windows handler
  thread is a deferred concern. v2 adds two specific
  sub-concerns: (a) handler thread observing the Arc<AtomicBool>
  before publication during `Runtime::new()` — must register
  AFTER the Arc is fully constructed and published; (b) Arc
  lifetime — `signal-hook` handles POSIX; Windows path needs an
  explicit static or leaked Arc so the handler thread can read
  after Runtime drop.
- **Cross-ADR 0023 interaction (NEW Risk #9).** Drop-initiated
  close in `FiberResponseBody::drop` enters `dispatch_until`,
  which observes `interrupt_pending` and raises Interrupt
  mid-close — repeats the ensure-leak Risk #1 of ADR 0023. ADR
  0023 v6 handed off two mitigation candidates: drain-before-
  close or no-interrupt-window. v2 picks the no-interrupt-window
  approach (`Vm::suppress_interrupt: bool`) — principled, reusable,
  same shape as `cext_depth` from ADR 0023.
- **Coordination with ADR 0024 — `dispatch_until` hot path
  (NEW).** Both ADRs modify `dispatch_until`'s top-of-loop. v2
  states the merge-order requirement: 0025 Phase 2 must land
  BEFORE 0024 Phase A (or 0024's first commit includes the 0025
  interaction). Interaction case: SIGINT during a synchronous
  Op::Yield's nested dispatch_until. Cross-link added.
- **`interrupt_pending` explicitly EXCLUDED from FiberSnapshot.**
  Vm-wide flag; a suspended Fiber's resume re-enters
  dispatch_until and sees the flag on its next op. Stated
  explicitly so a future implementer doesn't mistakenly add it
  to the snapshot table.

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

**Phase 0 — `Interrupt` exception class — SHIPPED 2026-05-29
(commit `a5337fd7`)**:

0. ✅ Install `Interrupt < SignalException < Exception` in
   `preamble/exceptions.rb` + matching BUILTIN_EXCEPTION_PARENT
   entries in `error.rs` + two embed tests verifying hierarchy
   walk and bare-rescue-doesn't-swallow.

**Phase 0.5 — `SystemExit` exception class (~1 commit, ADD in v2)**:

0.5. Install `SystemExit < Exception` in `preamble/exceptions.rb`
   + BUILTIN_EXCEPTION_PARENT entry. Required dependency for
   Phase 4 step 17 (`trap("INT") { exit }` → SystemExit → at_exit
   handlers). Decoupled from the rest like Phase 0.

   Placement note: SystemExit is `< Exception`, NOT under
   SignalException despite the "exit-on-signal" use case — CRuby
   draws the line because `Kernel#exit` (the normal source) is
   programmatic, not signal-driven. SignalException is reserved
   for SIG{TERM,HUP,...} shapes.

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
   `method_return` / `fiber_yield_pending` checks. **Memory ordering
   lockdown (v2)**: Relaxed-load + handler's SeqCst-store is
   sufficient for a SINGLE flag with no paired data. If a future
   change pairs additional state (e.g. signal-name discriminant,
   trap handler ObjId), upgrade the load to Acquire and pair the
   store with Release. Lock this contract in the comment so a
   future patch doesn't silently break the happens-before edge.
   When set: clear, honor `suppress_interrupt` if true (no-interrupt
   window, see Risk #9), otherwise trap with `RubyError::Interrupt`
   (new variant) or via the existing `RubyError::Uncaught` path
   with class_name "Interrupt".

   **Coordination with ADR 0024**: 0024 Phase A adds a synchronous
   Op::Yield wrapper that calls dispatch_until recursively. The
   interrupt_pending check fires on the INNER dispatch_until's
   top-of-loop too; the resulting Interrupt trap propagates
   through 0024's break-unwind helper (which uses the same
   rescues-stack walk method_return uses). Merge order: this
   Phase 2 commit must land BEFORE 0024 Phase A's first commit OR
   that commit must include this interaction. Test:
   `def f; yield; end; f { sleep(60) }` + SIGINT → Interrupt
   propagates out of `f`'s frame correctly.
6. C-ext re-entrancy guard: signal not delivered when
   `cext_depth > 0` (defer until cext returns). Mirrors existing
   `Fiber.yield` cext guard.
7. Fiber re-entrancy: a suspended Fiber's `interrupt_pending` is
   NOT delivered until the Fiber resumes. The flag lives on the Vm,
   not the Fiber — by design — so the suspended Fiber's resume
   re-enters dispatch_until and sees the flag on its next op.
   **`interrupt_pending` is explicitly EXCLUDED from FiberSnapshot's
   stash table** (ADR 0023 §"Fiber-scoped Vm state"). Vm-wide flag;
   stashing it would mean a Fiber suspended pre-signal could miss
   the interrupt on resume.
8. Subprocess tests: SIGINT during a CPU-bound loop → script
   raises `Interrupt`. Subprocess tests: trap from inside a method
   → backtrace points at the safe point, not the signal handler.

**Phase 3 — `Kernel#sleep` interruptible — BOTH no-args AND
with-args (~2 commits)**:

9. `Config::sleep_for` signature change: closure now takes
   `(Option<Duration>, &AtomicBool) -> Duration`. `None` Duration
   means sleep-forever-until-signal. CLI binary's closure does
   a polling loop with 50ms chunks. **v2 correction**: BOTH
   `sleep(secs)` AND `sleep` (no args) consult the flag — CRuby
   semantics: `sleep(10)` is interruptible too and returns elapsed
   integer seconds. The difference between Some(d) and None is
   only the upper bound on the polling loop, not whether the flag
   is checked.
10. `Kernel#sleep` (no args): call the closure with `None`, then
    raise Interrupt if the flag is set, otherwise return seconds
    slept (in the no-args case, this is the elapsed time before
    the interrupt fired — never the "forever" upper bound).
10a. `Kernel#sleep(secs)`: call the closure with `Some(Duration)`,
    then raise Interrupt if the flag fired before the upper bound,
    otherwise return Integer seconds requested (CRuby returns
    Integer seconds actually slept; rubyrs returns requested as
    lower bound — same shape as the pre-Phase 3 behavior).
11. Tests:
    - `sleep_default_raises_without_capability_injection` covers
      the no-capability case. Unchanged.
    - With capability + signal handler: SIGINT during `sleep`
      (no args) raises Interrupt within ~100ms; returned value
      from the catching `rescue` reflects elapsed seconds.
    - With capability + signal handler: SIGINT during `sleep(60)`
      raises Interrupt within ~100ms (not 60s); returned value
      from the catching `rescue` reflects elapsed seconds.
    - Without signal handler installed: existing `sleep(0.25)`
      capability test still passes (closure signature backward-
      compatible — flag exists but never set).

**Phase 4 — `Signal.trap("INT") { ... }` user handlers (~4-5 commits,
revised upward from v1's 3-4)**:

12. `Vm::signal_traps: HashMap<&'static str, SignalHandlerState>`
    where `SignalHandlerState` is `Default | Ignore | Block(ObjId)`.
    `Signal.trap(name, handler) → previous_handler` matches CRuby:
    - Input: `"DEFAULT"` / `"IGNORE"` / a `Proc` / an attached block
    - Returns: the PREVIOUSLY-installed handler in the same shape
      (`"DEFAULT"` / `"IGNORE"` / a Proc / nil if never set)
13. Safe-point check: if `interrupt_pending` is set, look up
    the trap for SIGINT.
    - `Block(b)`: invoke b at the safe point (re-entrant dispatch).
      Trap block runs on the main Fiber's frame stack — see Risk #8.
    - `Ignore`: clear flag, continue.
    - `Default`: raise Interrupt as before.
14. **`at_exit` × `trap("INT") { exit }`** (v2 addition). The
    canonical pattern: trap block calls `Kernel#exit` → raises
    SystemExit (requires Phase 0.5 SystemExit class). Existing
    SystemExit unwind path runs at_exit handlers — verify the
    integration. Test:
    ```ruby
    at_exit { puts "goodbye" }
    Signal.trap("INT") { exit 0 }
    sleep
    # SIGINT → exit → "goodbye\n" + clean exit.
    ```
15. **Trap block re-raising Interrupt (CRuby idiom).** The pattern
    `trap("INT") { raise Interrupt }` is sometimes used to force
    a deferred raise from a known-safe context. Test: trap block
    raises → propagates as a normal Vm exception, NOT swallowed.
16. Test: trap block runs at safe point, NOT in signal context.
    Backtrace points at the user code holding execution, not at
    the handler install site.

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

**Total**: ~12-16 commits over 3.5-4.5 weeks (revised upward from
v1's 10-13 / 3-4 weeks). Phase 0 already landed (`a5337fd7`).
Phase 0.5 added (~1 commit); Phase 3 expanded to cover sleep(secs)
interruptibility (~1 → 2 commits); Phase 4 expanded for the
correct Signal.trap return + at_exit/SystemExit path (~3-4 → 4-5
commits).

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
   (AtomicBool from any thread is fine), but two **v2-added**
   sub-concerns must be handled in the Windows install path:

   (a) **Arc publication ordering.** The handler thread observes
   the `Arc<AtomicBool>` via a static-like reference. The install
   must register AFTER the Arc is fully constructed and published
   (e.g. via `Arc::clone` into an `OnceLock<Arc<AtomicBool>>`
   stored at static lifetime). Otherwise the handler thread could
   observe a partially-published Arc — race condition.

   (b) **Arc lifetime past Runtime drop.** `signal-hook` handles
   this on POSIX (the static registration owns its Arc clone).
   Windows needs an explicit static `OnceLock<Arc<AtomicBool>>`
   or a deliberately leaked Arc so the handler thread can read
   without a dangling pointer after `Runtime` drop. Document
   that `install_signal_handler: true` implies a one-time per-
   process initialization that survives Runtime drops.

   The "single-threaded language semantics" claim (ADR 0015) is
   preserved at the *Ruby* level — the user can't observe the
   Windows handler thread from Ruby — but the host process now
   has a second OS thread for the lifetime of the program.
   `signal-hook` abstracts over POSIX; Windows needs its own
   `_install_signal_handler` arm. Defer Windows to a follow-up
   if needed; CLI users on Windows can document workarounds.

3. **Re-entrancy with c-ext frames.** Same shape as Fiber.yield's
   cext_depth guard — defer delivery until cext returns. Already
   solved pattern; no new risk.

4. **Signal arrival during `Runtime::new()` preamble eval.** The
   handler is installed BEFORE the preamble eval; if a SIGINT
   arrives during preamble compilation, the flag gets set, then
   the first user-script op consumes it. Could surprise the user
   ("why did my script raise Interrupt at start?"). **v2 mitigation
   picked**: drain the flag right before the user eval starts.
   `Runtime::eval` clears `interrupt_pending` once after the
   preamble path, before processing the user's source. Adds one
   atomic store to the eval entry — negligible cost.

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
   suspended, what happens? **v2 specification (was "proposal"
   in v1)**: the trap runs at the next safe point seen by
   `dispatch_until` — which is whichever Fiber (or root) is
   currently driving the Vm. With ADR 0023's single-Vm-mutex
   model, only one Fiber is dispatching at a time; that Fiber's
   frame stack is the trap's execution context. The
   suspended-but-not-resumed Fiber's state is untouched until
   its own resume. Stated precisely so a future Fiber + trap
   test in Phase 4 has a concrete oracle.

9. **Cross-ADR 0023 interaction — Drop-initiated close × interrupt
   flag (v2 ADD).** ADR 0023 v6 surfaced: `FiberResponseBody::drop`
   on client disconnect calls `invoke_body_close` which enters
   `dispatch_until`. With Phase 2's interrupt_pending check in
   place, SIGINT concurrent with disconnect would observe the
   flag inside the close path → raise Interrupt mid-close →
   repeat the ensure-leak shape of ADR 0023 Risk #1.

   **v2 mitigation picked**: add `Vm::suppress_interrupt: u32`
   (counter, mirroring `cext_depth`). `FiberResponseBody::drop`
   increments before invoking close, decrements after. The
   safe-point check honors the flag: if `suppress_interrupt > 0`,
   leave `interrupt_pending` set but don't act on it. Once the
   close finishes and `suppress_interrupt` returns to 0, the next
   safe point delivers the deferred interrupt.

   Why a counter not a bool: nested suppress windows. A close
   handler that itself runs through another close (rare but
   possible) needs counted suppression to avoid clearing the
   outer's window on the inner's exit.

   Reusable: any other "must-complete" cleanup path (future
   `at_exit` runner, ensure-block executor) uses the same
   counter. Documented as the canonical mechanism.

10. **Coordination with ADR 0024 — `dispatch_until` hot path (v2
    ADD)**. Both ADRs modify the top-of-loop. Phase 2's
    interrupt check + 0024's pending_yield handling share the
    same hot path. Specified merge order: 0025 Phase 2 lands
    FIRST (or 0024's Phase A first commit includes 0025's
    interaction). Interaction test (`def f; yield; end; f {
    sleep(60) }` + SIGINT → Interrupt propagates correctly) is
    OWNED BY 0024 Phase A but referenced here so the dependency
    is visible from both sides.

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

- **2026-05-29 — v2 (this revision).** Three parallel reviewer
  rounds (architecture / Rust safety / Ruby parity) on v1 surfaced
  six load-bearing corrections:
  - Phase 0 marked SHIPPED (commit `a5337fd7`).
  - Phase 0.5 added — `SystemExit < Exception` class needed for
    Phase 4's `trap("INT") { exit }` path.
  - `sleep(secs)` interruptibility (CRuby parity gap): BOTH
    `sleep` and `sleep(secs)` poll the flag; difference is only
    the polling upper bound.
  - `Signal.trap` return value corrected — CRuby returns the
    PREVIOUS handler, not the new one. Input/return shape
    `"DEFAULT" | "IGNORE" | Proc | block` → `"DEFAULT" | "IGNORE"
    | Proc | nil`.
  - `at_exit` × `trap("INT") { exit }` named — Phase 4 step 14
    threads through SystemExit unwind.
  - Memory ordering contract locked down: Relaxed-load OK for the
    single-flag case; pair upgrade required if future state
    pairs the flag.
  - Windows path: Arc publication ordering + lifetime past
    Runtime drop made concrete (Risk #2 expanded).
  - Risk #9 added: cross-ADR 0023 interaction (Drop-initiated
    close × interrupt flag). Mitigation picked:
    `Vm::suppress_interrupt: u32` counter, reusable for other
    must-complete cleanup paths.
  - Risk #10 added: coordination with ADR 0024 — both modify
    `dispatch_until` top-of-loop; merge order specified.
  - `interrupt_pending` explicitly EXCLUDED from FiberSnapshot.
  Total estimate bumped 10-13 → 12-16 commits / 3-4 → 3.5-4.5
  weeks. Status still Proposed; v2 marker added.
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
