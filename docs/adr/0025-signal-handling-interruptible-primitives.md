# 0025: Signal handling + interruptible Vm primitives

## Status

Proposed (2026-05-29). **v5** — Nice-to-have parity refinement
from the round-2 review: `sleep` interrupted wording reframed —
v2/v3/v4 said "returned value from rescue reflects elapsed
seconds," which is a parity drift (CRuby's sleep raises through
the call; elapsed is recovered by the user measuring
`Time.now` in the rescue). v4 + v3 critical fixes preserved.
No code change in this ADR. Phase 0 has SHIPPED (commit
`a5337fd7`).

**v2 → v3 changes**:

- **RAII guard for `suppress_interrupt`** (Rust-safety H2). v2
  added the counter but left bare increment/decrement. A panic
  in `invoke_body_close` would skip the decrement → suppress
  state stuck > 0 → interrupts permanently disabled for the
  Vm's remaining lifetime. v3 adds `SuppressInterruptGuard<'a>`
  with Drop-decrement (same shape as `YieldDepthGuard` in 0024
  v3).
- **`suppress_interrupt` placement choice locked**. Round 2
  asked: stash in FiberSnapshot (so Fiber-suspend-mid-close
  resumes with correct suppress) or trap Fiber.yield from close
  paths (mirror cext_depth's Fiber.yield guard)? v3 picks the
  TRAP approach: when `suppress_interrupt > 0` AND
  `Fiber.yield` is called, trap with FiberError. Simpler than
  stashing; matches existing cext_depth pattern; explicit
  contract that close paths don't yield. Documented in Risk #9
  + the suppress-window mechanism section.
- **OnceLock scope clarified**. v2 said
  `OnceLock<Arc<AtomicBool>>` for Windows install. Round 2
  asked: what if two Runtimes in the same process? v3 specifies
  `install_signal_handler: true` is a ONE-TIME PER-PROCESS op;
  second Runtime construction with the flag set returns
  `Err(AlreadyInstalled)` from `Runtime::with_config` (new
  error variant). Documented as a deliberate constraint, not a
  hidden cost.
- **Counter interaction matrix** (`cext_depth`,
  `yield_recursion_depth`, `suppress_interrupt`). v3 specifies
  the safe-point check ordering + per-counter semantics. All
  three counters are RAII-guarded (existing `cext_depth` audited
  + confirmed; the two new ones documented in v3).
- **Merge-order specification corrected** (mirror of 0024 v3's
  same correction). v2 said "0025 Phase 2 must precede 0024
  Phase A" — shorthand; Phase 2 can't literally land first
  without Phase 1 + flag/handler install. v3 specifies
  achievable order: Phase 0 (done) → Phase 0.5 → Phase 1 →
  Phase 2 → THEN 0024 Phase A.

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

**Phase 0.5 — `SystemExit` exception class + `Kernel#exit` family
(~2 commits, expanded in v4 from v2's 1-commit footprint)**:

0.5a. Install `SystemExit < Exception` in `preamble/exceptions.rb`
   + BUILTIN_EXCEPTION_PARENT entry. Includes the CRuby attrs:
   - `status: Integer` — the exit status. Constructor accepts
     `Integer | true | false | nil`: true → 0, false → 1, nil → 0,
     Integer → as-is.
   - `success? -> Bool` — `status == 0`.

   Concrete preamble shape (matches CRuby 3.x):
   ```ruby
   class SystemExit < Exception
     def initialize(*args)
       case args.length
       when 0 then @status = 0; super("SystemExit")
       when 1
         case args[0]
         when Integer then @status = args[0]; super("exit")
         when true    then @status = 0; super("exit")
         when false   then @status = 1; super("exit")
         when nil     then @status = 0; super("exit")
         else              @status = 0; super(args[0].to_s)
         end
       when 2
         # (Integer, msg)
         @status = args[0]; super(args[1])
       end
     end
     attr_reader :status
     def success?; @status == 0; end
   end
   ```

   Placement note: SystemExit is `< Exception`, NOT under
   SignalException despite the "exit-on-signal" use case — CRuby
   draws the line because `Kernel#exit` (the normal source) is
   programmatic, not signal-driven. SignalException is reserved
   for SIG{TERM,HUP,...} shapes.

0.5b. **`Kernel#exit` / `Kernel#exit!` / `Kernel#abort` family
   (v4 ADD)**. Round 2 surfaced these as omissions; v4 specifies:

   - **`Kernel#exit(status = true)`**: raises `SystemExit.new(status)`.
     Normal exception unwind, so `ensure` blocks and `at_exit`
     handlers fire. Builtin in `vm/kernel.rs`'s `builtin_call`
     match — symmetric with `sleep`/`puts`.

   - **`Kernel#exit!(status = false)`**: immediate process exit
     via `std::process::exit(status as i32)`. SKIPS `ensure` and
     `at_exit`. Same Tier 1 capability gate as `sleep_for` —
     embed users opt in via a new `Config::process_exit:
     Option<Arc<dyn Fn(i32) + Send + Sync>>`. CLI binary wires
     `std::process::exit`; library default is `None`, in which
     case `exit!` raises RuntimeError ("Kernel#exit! requires
     Config::process_exit injection"). Avoid std::process::exit
     hardcoded so embedders can intercept (test hosts, language
     bindings).

   - **`Kernel#abort(msg = nil)`**: print `msg` (or `$!.message`
     if currently in an `at_exit`) to stderr, then `exit(1)`.
     Pure builtin in terms of `exit` + stderr write. No new
     capability — stderr write already supported via OutputSink
     (ADR 0021).

   `at_exit { ... }` handler stack lives separately on the Vm
   (`Vm::at_exit_handlers: Vec<ObjId>` block IDs). Phase 4 step
   14's SystemExit unwind path drains the stack LIFO, invoking
   each block. Decoupled from signal handling — `at_exit` is
   useful without `Signal.trap` (registered with `Kernel#at_exit
   { block }`).

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
10. `Kernel#sleep` (no args): call the closure with `None`. If
    the flag was set during the polling loop, raise Interrupt
    (the call never returns a value — the exception unwinds the
    sleep). Without signal handling, the no-args form is the
    documented ArgumentError per the existing `Kernel#sleep`
    Tier 1 behavior — only `install_signal_handler: true` makes
    sleep-forever meaningful.
10a. `Kernel#sleep(secs)`: call the closure with `Some(Duration)`.
    Two outcomes:
    - Duration elapsed without interrupt: return Integer seconds
      requested (CRuby returns Integer seconds actually slept;
      rubyrs returns requested as conservative lower bound —
      same shape as the pre-Phase 3 behavior).
    - Flag set before duration elapsed: raise Interrupt (the
      call does NOT return). User recovers elapsed time via
      `Time.now` measurement around the rescue, matching CRuby.
11. Tests:
    - `sleep_default_raises_without_capability_injection` covers
      the no-capability case. Unchanged.
    - With capability + signal handler: SIGINT during `sleep`
      (no args) raises Interrupt within ~100ms. **v5 reframed
      (round 2 parity correction)**: `sleep` does NOT return a
      value on interrupt — the Interrupt unwinds the call. The
      catching `rescue` block sees the Interrupt exception, not
      a return value. Test shape:

      ```ruby
      start = Time.now
      begin
        sleep        # interrupted by SIGINT
      rescue Interrupt
        elapsed = Time.now - start
        # elapsed ≈ 0.1s; sleep itself didn't return.
      end
      ```

      v2–v4 said "returned value from rescue reflects elapsed
      seconds." That was a parity drift — CRuby raises through
      sleep; elapsed is recovered by the user measuring
      Time.now in the rescue.
    - With capability + signal handler: SIGINT during `sleep(60)`
      raises Interrupt within ~100ms (not 60s). Same shape as
      above — sleep raises, user measures elapsed in rescue.
    - Without signal handler installed: existing `sleep(0.25)`
      capability test still passes (closure signature backward-
      compatible — flag exists but never set).
    - **Uninterrupted sleep return value (CRuby parity)**:
      `sleep(2)` running to completion still returns Integer 2
      (the current Phase 3 / pre-Phase 3 behavior — flag
      machinery doesn't change the no-interrupt return path).

**Phase 4 — `Signal.trap("INT") { ... }` user handlers (~4-5 commits,
revised upward from v1's 3-4)**:

12. `Vm::signal_traps: HashMap<i32, SignalHandlerState>` keyed by
    Unix signal number (SIGINT=2, SIGTERM=15, etc.) where
    `SignalHandlerState` is `Default | Ignore | Block(ObjId)`.
    `Signal.trap(name, handler) → previous_handler` matches CRuby.

    **v4 — signal-name normalization (round 2 surfaced)**.
    CRuby accepts:
    - `"INT"` (bare short name)
    - `"SIGINT"` (with prefix)
    - `:INT` / `:SIGINT` (Symbol form, either with/without prefix)
    - `2` (Integer signal number)

    v4 normalizes via a `parse_signal_name` helper:
    ```rust
    fn parse_signal_name(v: &Value) -> Option<i32> {
        match v {
            Value::Int(n) if (1..=64).contains(n) => Some(*n as i32),
            Value::Sym(id) => parse_str(interner.resolve(*id)),
            Value::Str(s) => parse_str(&s.to_string_lossy()),
            _ => None,
        }
    }
    fn parse_str(s: &str) -> Option<i32> {
        let trimmed = s.strip_prefix("SIG").unwrap_or(s);
        match trimmed {
            "HUP" => Some(1), "INT" => Some(2), "QUIT" => Some(3),
            "ILL" => Some(4), "TRAP" => Some(5), "ABRT" => Some(6),
            "FPE" => Some(8), "KILL" => Some(9), "USR1" => Some(10),
            "SEGV" => Some(11), "USR2" => Some(12), "PIPE" => Some(13),
            "ALRM" => Some(14), "TERM" => Some(15), "CHLD" => Some(17),
            "CONT" => Some(18), "STOP" => Some(19), "TSTP" => Some(20),
            "TTIN" => Some(21), "TTOU" => Some(22), "URG" => Some(23),
            "WINCH" => Some(28),
            // Subset; expand as Phase 4 lands more handlers.
            _ => None,
        }
    }
    ```
    Unknown signal name → ArgumentError (matches CRuby:
    `"unsupported signal SIG…"`).

    Handler input shape:
    - `"DEFAULT"` / `:DEFAULT` → `SignalHandlerState::Default`
    - `"IGNORE"` / `:IGNORE` / `"SIG_IGN"` → `SignalHandlerState::Ignore`
    - A `Proc` (`Value::Block`) → `SignalHandlerState::Block(obj_id)`
    - An attached block via `&block` → same as Proc
    - Nil → ArgumentError (matches CRuby).

    Return shape: PREVIOUSLY-installed handler — same shape:
    - `SignalHandlerState::Default` → `"DEFAULT"` string
    - `SignalHandlerState::Ignore` → `"IGNORE"` string
    - `SignalHandlerState::Block(id)` → `Value::Block(id)`
    - No previous entry (first install) → `"DEFAULT"` string
      (CRuby returns the default behavior name).

    **Subset scope (v4)**: Phase 4 implements `parse_str` for the
    Tier-1 portable signal set listed above. Real-time signals
    (SIGRTMIN+, etc.) and platform-specific names deferred.
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

**Total**: ~14-18 commits over 4-5 weeks (revised upward from v3's
12-16 / 3.5-4.5 weeks). Phase 0 already landed (`a5337fd7`).
Phase 0.5 expanded 1 → 2 commits (SystemExit class + exit/exit!/
abort family). Phase 3 stays 2 commits. Phase 4 stays 4-5 commits.

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
   without a dangling pointer after `Runtime` drop.

   **v3 — two-Runtime case (round 2 surfaced)**. If a host
   embeds two `Runtime`s in the same process with
   `install_signal_handler: true`, the second's `OnceLock::set`
   would conflict. v3 design choice: `install_signal_handler:
   true` is a ONE-TIME PER-PROCESS operation. Second Runtime
   construction with the flag set returns a new
   `RuntimeBuildError::SignalHandlerAlreadyInstalled` from
   `Runtime::with_config`. Hosts wanting two Runtimes share the
   handler installation explicitly: install once via a
   first-time `Runtime` (or a dedicated init call), then
   construct subsequent Runtimes with `install_signal_handler:
   false` (they still consume the shared flag via Arc clone
   inside the OnceLock). Document in the embedding guide; not a
   silently-shared cross-Runtime contract.

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

   **v2 mitigation picked + v3 RAII guard**: add
   `Vm::suppress_interrupt: u32` (counter, mirroring `cext_depth`).
   Wrap mutation in `SuppressInterruptGuard<'a>` whose `Drop`
   decrements, so a panic in `invoke_body_close` does NOT leak the
   counter and permanently disable interrupts:

   ```rust
   struct SuppressInterruptGuard<'a> { vm: &'a mut Vm }
   impl<'a> SuppressInterruptGuard<'a> {
       fn enter(vm: &'a mut Vm) -> Self {
           vm.suppress_interrupt += 1;
           Self { vm }
       }
   }
   impl Drop for SuppressInterruptGuard<'_> {
       fn drop(&mut self) { self.vm.suppress_interrupt -= 1; }
   }
   ```

   `FiberResponseBody::drop` enters the guard before invoking
   close; the guard's Drop fires on close return (normal or
   panic), restoring the counter cleanly. The safe-point check
   honors the flag: if `suppress_interrupt > 0`, leave
   `interrupt_pending` set but don't act on it. Once close
   finishes and the guard's Drop decrements to 0, the next safe
   point delivers the deferred interrupt.

   Why a counter not a bool: nested suppress windows. A close
   handler that itself runs through another close (rare but
   possible) needs counted suppression to avoid clearing the
   outer's window on the inner's exit.

   **v3 — `suppress_interrupt` placement decision (round 2
   surfaced)**: should `suppress_interrupt` stash in FiberSnapshot
   (so Fiber-suspend-mid-close restores it on resume) or should
   close paths trap on `Fiber.yield` (mirror the `cext_depth`
   Fiber.yield guard from ADR 0023)?

   v3 picks the **trap-on-yield** approach. Rationale:
   - Mirrors the existing `cext_depth` pattern — close paths and
     C-ext frames both run cleanup code that fundamentally
     shouldn't suspend mid-flight.
   - Simpler than stashing: no FiberSnapshot table edit, no
     resume-time restoration ambiguity.
   - User-visible contract: "a Rack body's `close` method MUST
     NOT call `Fiber.yield`." Existing Rack 3 close handlers
     don't yield (they're cleanup, not iteration).
   - Trap shape: when `suppress_interrupt > 0` AND `Fiber.yield`
     is called, raise `FiberError("can't yield from close
     handler")`. Existing cext guard already uses
     `FiberError("can't yield from cext")`; symmetric.

   Therefore `suppress_interrupt` is **Vm-wide**, **NOT stashed
   in FiberSnapshot** — same as `cext_depth`. Confirmed
   explicitly in ADR 0023 v7's stash table.

   Reusable: any other "must-complete" cleanup path (future
   `at_exit` runner, ensure-block executor) uses the same
   counter AND inherits the no-Fiber-yield contract. Documented
   as the canonical mechanism.

   **Counter interaction matrix** (round 2 raised this).
   rubyrs's safe-point check now consults THREE Vm-wide counters:

   | Counter | Source ADR | Suppresses what |
   |---|---|---|
   | `cext_depth` | 0023 | `Fiber.yield` |
   | `yield_recursion_depth` | 0024 | `Op::Yield` (caps recursion, traps ResourceExhausted on overflow) |
   | `suppress_interrupt` | 0025 | `interrupt_pending` delivery + `Fiber.yield` from close paths |

   All three are Vm-wide, NOT stashed in FiberSnapshot, and
   RAII-guarded via Drop-decrement helpers. Safe-point check
   ordering (in `dispatch_until` top-of-loop):

   1. `method_return.is_some()` — already exists. Returns early.
   2. `fiber_yield_pending.is_some()` — already exists. Returns
      early.
   3. NEW: `interrupt_pending.load(Relaxed)` && `suppress_interrupt == 0`
      → clear flag, trap Interrupt. (When `suppress_interrupt > 0`,
      flag stays set; checked again at the next safe point.)
   4. Existing fuel decrement.

   `cext_depth` and `yield_recursion_depth` aren't part of the
   safe-point ordering — they're consulted at their respective
   trap sites (Fiber.yield call, Op::Yield entry).

10. **Coordination with ADR 0024 — `dispatch_until` hot path.**
    Both ADRs modify the top-of-loop. Phase 2's interrupt check
    + 0024's pending_yield handling share the same hot path.

    **v3 corrected merge order** (v2 said "Phase 2 lands first" —
    shorthand; Phase 2 can't literally land first without Phase 1
    + flag/handler install). The actual achievable order:

    1. Phase 0 — Interrupt class hierarchy. **SHIPPED** (commit
       `a5337fd7`).
    2. Phase 0.5 — SystemExit class. Independent; can land any
       time before Phase 4.
    3. Phase 1 — `interrupt_pending` flag + Config capability +
       signal-hook handler.
    4. Phase 2 — safe-point check in `dispatch_until` (the actual
       hot-path edit).
    5. THEN ADR 0024 Phase A — synchronous Op::Yield +
       break-unwind + the cross-ADR interaction handling.

    Interaction test (`def f; yield; end; f { sleep(60) }` +
    SIGINT → Interrupt propagates correctly) is OWNED BY 0024
    Phase A but referenced here so the dependency is visible from
    both sides.

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

- **2026-05-29 — v5 (this revision).** Nice-to-have parity
  refinement: Phase 3 step 10 + step 11 test descriptions
  reframed for CRuby-faithful sleep-interrupt semantics. v2/v3/
  v4 said "returned value from rescue reflects elapsed seconds";
  CRuby actually raises through sleep and the catching rescue
  sees the Interrupt exception — elapsed time is recovered by
  the user measuring `Time.now` around the rescue, not by sleep
  returning a value. Test shape rewritten with the canonical
  `start = Time.now; begin; sleep; rescue Interrupt; Time.now -
  start; end` pattern. Uninterrupted sleep return value (Integer
  seconds) unchanged.
- **2026-05-29 — v4.** Important-tier parity
  refinements from the round-2 review:
  - Phase 0.5 expanded with concrete preamble shape:
    `SystemExit#status` (Integer; constructor accepts
    Integer|true|false|nil), `#success?` (Bool from
    `status == 0`). Matches CRuby 3.x exactly.
  - New Phase 0.5b: `Kernel#exit` (raises SystemExit) /
    `Kernel#exit!` (immediate, skips at_exit + ensure; requires
    new `Config::process_exit` capability) / `Kernel#abort`
    (stderr write + exit(1)). Phase 0.5 footprint 1 → 2 commits.
  - `Vm::at_exit_handlers: Vec<ObjId>` stack named — decoupled
    from signal handling, used by SystemExit unwind path.
  - Signal.trap signal-name normalization: keyed by Unix signal
    number (`i32`), `parse_signal_name` helper accepts String /
    Symbol / Integer / `"SIG…"` prefix variants. Tier-1 portable
    signal subset enumerated; real-time signals deferred.
  - Handler input/return shapes nailed down to the exact CRuby
    string ("DEFAULT" / "IGNORE") + Proc + nil contract.
  Total estimate 12-16 → 14-18 commits / 3.5-4.5 → 4-5 weeks.
- **2026-05-29 — v3.** Second-round review on v2
  surfaced four remaining issues, all closed inline:
  - `suppress_interrupt` counter now wrapped in
    `SuppressInterruptGuard<'a>` with Drop-decrement (panic-safe).
    Mirrors `YieldDepthGuard` in 0024 v3 and ADR 0013's
    `VmPtrGuard`.
  - `suppress_interrupt` placement question (FiberSnapshot stash
    vs no-yield trap) resolved: pick TRAP. Close paths trap on
    `Fiber.yield` via the same `FiberError` shape `cext_depth`
    already uses. Counter stays Vm-wide; no FiberSnapshot edit.
    Documented as a user-facing contract.
  - OnceLock two-Runtime case: `install_signal_handler: true` is
    one-time per process; second Runtime returns
    `RuntimeBuildError::SignalHandlerAlreadyInstalled`. Document
    + provide explicit shared-Arc pattern for embed users.
  - Counter interaction matrix specified: three Vm-wide counters
    (`cext_depth`, `yield_recursion_depth`, `suppress_interrupt`),
    all RAII-guarded, safe-point check order documented.
  - Merge-order corrected (symmetric with 0024 v3's mirror):
    Phase 0 (done) → 0.5 → 1 → 2 → THEN 0024 Phase A.
  No estimate change from v2 (still 12-16 commits / 3.5-4.5 weeks)
  — the v3 changes refine specifications without adding new phases.
- **2026-05-29 — v2.** Three parallel reviewer
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
