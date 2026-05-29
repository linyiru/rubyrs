# 0024: Bytecode-level iter drivers + block-break propagation through `Op::Yield`

## Status

Proposed (2026-05-29). **v3** — second-round review on v2 surfaced
three remaining issues (RAII guard for `yield_recursion_depth`,
merge-order specification was a shorthand not actual order, FiberSnapshot
stash table update dependency on ADR 0023 not visible). All addressed
in v3 inline. No code change in this ADR — implementation lands in a
follow-up after the design is accepted.

**v2 → v3 changes**:

- **RAII guard for `yield_recursion_depth`** (Rust-safety H1). v2
  added the counter but left increment/decrement bare. A panic
  through the synchronous Op::Yield wrapper would skip the
  decrement and leak counter monotonically — eventually false-
  tripping `Config::max_yield_recursion`. v3 specifies
  `YieldDepthGuard<'a>` with `Drop` decrementing, same shape as
  ADR 0013's `VmPtrGuard` and ADR 0023 v2's `FiberStashGuard`.
- **FiberSnapshot stash dependency on ADR 0023 explicit**. v2
  said the new `Frame::pending_yield` "MUST be added to the v2
  FiberSnapshot table" — but didn't note this is a cross-ADR
  edit that ADR 0023 v7 has to make. v3 explicitly states the
  dependency + lists `pending_yield` as a v3 entry for ADR 0023's
  stash table.
- **Merge-order specification corrected**. v2 said "0025 Phase 2
  must precede 0024 Phase A's first commit." That's shorthand —
  Phase 2 can't literally land first without Phase 1 (flag +
  capability + signal handler install) preceding it. v3 specifies
  the achievable order: 0025 Phase 0 (shipped) → Phase 0.5
  (SystemExit) → Phase 1 → Phase 2 → THEN 0024 Phase A. Stated
  symmetrically with 0025 v3's mirror update.

**v1 → v2 changes**:

- Risk #1 (Rust-stack bound) — **load-bearing safety argument was
  wrong**. v1 said `max_fiber_frame_depth` covers Rust-stack growth
  "transitively". It does not: `max_fiber_frame_depth` bounds Vm
  frame depth, but recursive `Op::Yield → dispatch_until → step →
  Op::Yield` grows the Rust stack on a *single yield site chain*,
  independent of Vm frame growth. v2 adds an explicit
  `yield_recursion_depth` counter bounded by a new
  `Config::max_yield_recursion` cap, mirroring the existing
  resource-cap pattern.
- Risk #2 (IP advancement) — was an unresolved design hole, not a
  risk. v2 **picks option (ii)**: a per-frame `pending_yield: bool`
  flag set on Op::Yield entry, cleared on yield's normal return.
  On Fiber resume, dispatch sees the flag and treats the IP as
  "yield-in-progress" rather than re-recursing. Promoted from "two
  options offered" to "chosen mechanism with stated trade-off."
- Alt 2 weighting — v2 acknowledges Alt 2 (frame-marking) is the
  closer comparable now that option (ii) of Risk #2 adds a per-
  frame flag anyway. Rejection rationale rewritten honestly: chose
  Op::Yield synchronous-with-flag over Alt 2's frame-marking
  because it ALSO unblocks `Kernel#loop` (which Alt 2 doesn't), and
  Phase B benefits from the same flag mechanism. Commit estimate
  bumped from 5-7 to 7-9 for Phase A.
- Ruby parity gaps surfaced by review:
  * Block-break target: v1 said "nearest non-block frame"; CRuby
    target is the YIELDING method (which is the nearest non-block
    frame *in the simple case* but diverges with nested-yield-
    through-block-method chains). v2 names this honestly + defers
    full CRuby parity to Phase A round 2 with a concrete test list.
  * `ensure` on block-break: v1 said "thread through
    begin_loop_transfer". v2 adds the concrete mechanism (block-
    break uses the same `rescues` stack walk as method_return) +
    pins it with a Phase A test.
  * StopIteration / `Kernel#loop`: v1 said "moot". v2 pins
    `Kernel#loop` Phase A def will explicitly rescue StopIteration,
    matching CRuby exactly. Decoupled from the bigger Enumerator/
    Lazy work.
  * Hash#each insertion order: v2 names the bytecode iter form
    must walk the ordered entry vector, not bucket order.
  * Enumerable#map break-with-value: v2 names that `map`'s
    accumulator is replaced (not appended-to) by `break val`,
    matching CRuby.
- Coordination with ADR 0025: both ADRs touch `dispatch_until`'s
  top-of-loop. v2 adds explicit merge-order note + interaction
  case (interrupt-during-synchronous-yield).

Two related Fiber-completeness gaps surfaced as ADR 0023 v4 follow-ups
([ADR 0023](0023-true-async-streaming.md) §"Remaining Phase 2 polish"
and §"User-facing remediation"):

- **Fiber + Rust-level iter drivers** (`Int#times`, `upto`, `downto`,
  `Array#each`, `Range#each`, `Hash#each`, `Enumerable.map`, etc.):
  silent truncation when the block yields to a Fiber. Pinned by
  `p2_21_known_bug_times_loop_inside_fiber_yield`. Current
  mitigation (`step_block` no-ops on pending Fiber yield) prevents
  data corruption but only delivers the first iteration's chunk.
- **`Kernel#loop` not installable as a Ruby def**: `break` inside the
  block doesn't propagate through `Op::Yield`, so `def loop; while
  true; yield; end; end` hangs on `loop { break }`. Documented in
  `crates/rubyrs/src/preamble/object.rb`. Three implementation paths
  enumerated there; this ADR consolidates the recommendation.

Both gaps share a common root cause: rubyrs's `Op::Yield` is
"fire-and-forget" — it pushes the block frame and returns. The block
runs in subsequent dispatch iterations; control flow signals from the
block (break, Fiber.yield) are only recovered by Rust-level callers
that explicitly call `step_block`. Ruby-level methods that `yield`
from bytecode have no recovery path.

## Context

### Op::Yield's current semantics

`Op::Yield(argc)` in `vm/step.rs`:

```rust
let block = self.frames.iter().rev()
    .find(|f| !f.is_block)
    .and_then(|f| f.block_arg);
let block = match block { Some(b) => b, None => return Err(...) };
let argc = argc as usize;
let split = self.stack.len() - argc;
let args: Vec<Value> = self.stack.drain(split..).collect();
self.invoke_block(block, args)?;
```

`invoke_block` pushes a new block frame and returns. The Op::Yield
match arm falls through; the dispatch loop's next iteration runs the
block's bytecode. When the block does `Op::Break + Op::Return`:

- `Op::Break` sets `vm.break_signaled = true`
- `Op::Return` pops the block frame, leaves the value on the stack

Control resumes in the yielding method's bytecode at the IP after
Op::Yield. **Nothing checks `break_signaled` at this point.** The
yielding method's next op runs normally. For `def loop; while true;
yield; end; end`, the `while` loops back to yield again — infinite
loop.

### The Rust-driver recovery path that does work

`vm::iter::step_block` (the Rust helper iter drivers use) calls
`invoke_block` then `dispatch_until(pre_frames)`, waits for the
block to fully unwind, then checks `break_signaled` and returns
`BlockStep::Break(val)`:

```rust
self.invoke_block(block, args)?;
self.dispatch_until(pre_frames)?;
if self.method_return.is_some() { return Ok(BlockStep::MethodReturn); }
let r = self.stack.pop().unwrap_or(Value::Nil);
if self.break_signaled {
    self.break_signaled = false;
    return Ok(BlockStep::Break(r));
}
Ok(BlockStep::Value(r))
```

The Rust loop in `Int#times` reads BlockStep and breaks its for-loop.
Method returns. Clean.

### Why Fiber + Rust-iter doesn't compose

The Rust for-loop in `Int#times`-style drivers holds the iteration
counter on the Rust call stack. When the block calls Fiber.yield:

- `vm.fiber_yield_pending` gets set
- `dispatch_until` sees the flag at top of loop and returns
- Control returns up the Rust stack: step_block → for-loop → ?

The Rust stack from the original Fiber.resume call site is gone by
the time the Fiber resumes (resume re-enters via a different path).
Iter state on the Rust stack is lost. The
[ADR 0023 P2 #21 follow-up](0023-true-async-streaming.md) ships a
silent-truncation guard in `step_block` (no-op subsequent calls when
yield_pending is set) to prevent the pre-fix corruption pattern
(`0, 4, 4, 4, 4`), but cannot RECOVER the missed iterations.

## Decision

**Replace `Op::Yield`'s fire-and-forget pattern with synchronous
block execution + break propagation; replace Rust-level iter drivers
with bytecode-level equivalents.**

The two changes are interlocking:

1. **`Op::Yield` becomes synchronous (Phase A).** The match arm calls
   `invoke_block` then `dispatch_until(pre_frames)` to drive the
   block to completion, mirroring `step_block`. After dispatch_until
   returns, the arm checks `break_signaled`. If set, the yielding
   method unwinds via the **break-unwind helper**: pop frames up to
   the YIELDING method's frame (typically the nearest non-block, but
   see Risk #4 for the CRuby-faithful target spec when nested-yield
   chains complicate "nearest non-block"), push the break value,
   clear `break_signaled`. The unwind walks the same `rescues` stack
   that `method_return` uses, so `ensure` blocks in the yielding
   method fire on break — matches CRuby.

   The recursive `dispatch_until` inside Op::Yield is the same
   `&mut Vm` continued down the Rust call stack, NOT a new reborrow.
   ADR 0013's no-overlapping-`&mut Vm` contract holds — a future
   reader must not "fix" this with a fresh `with_vm_ptr_set`.
   Bounded by a new `yield_recursion_depth` counter (Risk #1) +
   `Config::max_yield_recursion`. The `cext_depth` Fiber.yield
   guard from ADR 0023 transitively applies — the inner
   `dispatch_until` honors the same guard because it shares Vm state.

   **IP-advancement semantics (Risk #2, resolved)**: each Frame
   gains a `pending_yield: bool`. Op::Yield SETS it before
   invoke_block, CLEARS it after dispatch_until returns normally.
   On Fiber yield mid-block, dispatch_until exits with
   pending_yield still SET on the yielding-method frame. Fiber
   resume restores frames including pending_yield=true; the IP at
   the yielding method is still at Op::Yield. Dispatch sees
   pending_yield=true at frame entry and SKIPS invoke_block + the
   block-frame setup (already on the stack), going straight to the
   "dispatch_until-just-returned" branch. No re-recursion. The flag
   is cleared and IP advances. **Per-Frame field, NOT Vm-wide** —
   nested concurrent yields each track their own pending state.

2. **Rust-level iter drivers become bytecode helpers (Phase B).**
   `Int#times` and friends compile to a small bytecode template
   (`while i < n; yield i; i += 1; end`) rather than a Rust for-loop
   with `step_block`. The bytecode form puts the iteration counter
   in Vm frame locals, which FiberStashGuard already snapshots, so
   Fiber yield naturally resumes mid-iteration. `Hash#each` walks
   the HashObj's ordered entry vector (CRuby 1.9+ insertion-order
   guarantee). `Enumerable#map`'s accumulator is a frame-local
   Array; `break val` REPLACES the accumulator with `val` (matches
   CRuby: `[1,2,3].map { |x| break "early" } # => "early"`, NOT
   `[]`).

Phase A unblocks `Kernel#loop` (as a Ruby def) and is the smaller of
the two. Phase B is the permanent Fiber+iter fix.

### Why this over the alternatives

Considered and rejected:

**Alt 1 — Rust-builtin `loop`.** Fixes `Kernel#loop` but inherits the
silent-truncation guard for Fiber bodies (same UX as `times`). Doesn't
address the broader iter-driver gap. Adds another bespoke `do_call_block`
no-recv arm. Mitigation that doesn't generalize.

**Alt 2 — Mark block frames "Rust-driver-above" vs "yield-from-Ruby"
so Op::Return knows whether to propagate break further.** Would let
break propagation live in Op::Return without changing Op::Yield's
fire-and-forget. Now that the chosen approach ALSO adds a per-Frame
flag (`pending_yield` from Risk #2 resolution), the cost difference
narrows considerably — Alt 2's "wiring update at every invoke_block
callsite" criticism applies in mirror to the chosen approach. Three
reasons the chosen approach still wins:

1. **`Kernel#loop` unblocked.** Alt 2 keeps Op::Yield fire-and-
   forget, so break inside a Ruby-defined yielding method (which is
   exactly what `Kernel#loop` is) doesn't propagate. The chosen
   approach handles `Kernel#loop` natively.
2. **Phase B groundwork.** Bytecode iter drivers naturally want
   synchronous yield semantics so the iter's "what value did the
   block produce" is on the stack right after Op::Yield. Alt 2
   keeps the asynchronous shape, complicating accumulator handling.
3. **Single mechanism for both fix targets.** Alt 2 fixes break
   propagation but leaves the Fiber+Rust-iter silent-truncation
   problem untouched (Phase B still required). The chosen approach
   solves both with one structural change.

Alt 2 is a defensible compromise that ships break propagation
without committing to Phase B; rejected because the project is
already committed to addressing the silent-truncation gap, making
the structural change worth the up-front investment.

**Alt 3 — Leave the bugs documented forever.** Both already are:
ignored regression test plus preamble doc-block. But the limitation
makes `Kernel#loop` un-installable and locks every Fiber-streaming
body into the `while`-counter idiom forever. Acceptable as a stopgap;
not acceptable as a destination.

## Implementation plan

**Phase A — Op::Yield synchronous + block-break propagation
(~7-9 commits, revised upward from v1's 5-7 after the Risk #1 +
Risk #2 resolutions added a per-Frame field + per-Vm counter)**:

1. Add `Frame::pending_yield: bool` (Risk #2) +
   `Vm::yield_recursion_depth: u32` + `Config::max_yield_recursion:
   Option<u32>` (Risk #1) + `YieldDepthGuard<'a>` (Risk #1 RAII).
   Wire into Frame construction. **Cross-ADR edit**: this commit
   ALSO updates ADR 0023 §"Fiber-scoped Vm state" — adds
   `pending_yield` to the "must stash" rows; adds
   `yield_recursion_depth` to the "DO NOT stash" rows (Vm-wide,
   not Fiber-scoped — same as `cext_depth`). ADR 0023 v7 already
   carries placeholder rows for these; this commit fills them in
   when the code lands.
2. `Op::Yield` synchronous variant: SET pending_yield, invoke
   block, dispatch_until to block-frame depth, check method_return
   + break_signaled, CLEAR pending_yield, advance IP. Behavior-
   preserving for the no-break case — all existing yield tests
   must stay green. Coordination note inline: this commit must
   land AFTER ADR 0025 Phase 2's `interrupt_pending` check, OR
   include the 0025 interaction handling.
3. Resume path: dispatch entry checks `frame.pending_yield`; if
   set, skip invoke_block (block frame already on stack) and go
   to the post-block branch. Test: Fiber + yield + suspend +
   resume yields correctly.
4. Block-break unwind helper: when `break_signaled` is set after
   dispatch_until, walk `rescues` stack on the yielding-method
   frame (same path as method_return) — ensure blocks fire —
   then truncate stack and push break value. Clear break_signaled.
5. Test: `def loop; while true; yield; end; end` + `loop { break }`
   exits cleanly + ensure clause inside `loop` fires (Risk #7).
6. Test: `loop { break val }` returns val.
7. Test: nested yield (`def f; xs.each { |x| yield x }; end; f { break }`)
   propagates correctly to f's caller.
8. Test: Rust-iter break (`Int#times`) still works — verifies the
   Op::Yield path doesn't interfere with step_block's existing
   handling.
9. Install `def loop` in `preamble/object.rb` matching the v2
   CRuby-faithful form (with `rescue StopIteration` per Risk #8).
   Top-level def (rubyrs's top-level dispatch walks
   `toplevel_methods`). Verifies the integration end-to-end.

**Phase B — bytecode iter drivers (~10-15 commits)**:

8-9. Replace `Int#times` with a bytecode template emitted at compile
   time. The compiler recognizes `n.times { |i| ... }` and emits a
   `while`-counter loop inline. Falls back to Rust step_block for
   the no-literal-block form or non-`Int` receivers.
10. Same treatment for `Int#upto` / `downto`.
11. `Array#each` / `Hash#each` / `Range#each` — bytecode loops over
   bytecode-driven iterators.
12. `Enumerable#map` / `select` / `reject` / `reduce` — same
   transformation, accumulator in a Vm local.
13. Remove `step_block`'s fiber_yield_pending guard from
   `vm/iter.rs` (P2 #21 follow-up). With bytecode iter drivers, the
   guard becomes unreachable.
14. Update `p2_21_known_bug_times_loop_inside_fiber_yield` to require
   all five chunks. Promote the test name from "known_bug" → the
   permanent regression name.
15. Revisit SSE example: replace `while`-counter pattern with
   natural `times` (CRuby idiom).
16-17. Slack for review iterations + perf-regression hunting (bytecode
   iter MIGHT be slower than the Rust for-loop for non-Fiber cases;
   benchmark and decide whether to keep the Rust path as a
   non-_fiber fast path).

**Total**: ~17-24 commits over 5-7 weeks (revised upward from v1's
15-22 / 4-6 weeks). Phase A is ~1.5 weeks; Phase B is the bulk.

## Risks + open questions

1. **Op::Yield recursion depth.** Synchronous yield re-enters
   `dispatch_until` recursively from within `step`. Existing
   `step_block` callers use the same pattern, but step_block is
   only called from Rust iter drivers — a single level of recursion
   per iter call. With Op::Yield synchronous, ANY Ruby method that
   yields adds Rust frames. A method that yields, where the block
   calls a method that yields, etc., adds ~2-3 Rust frames per
   yield-site nesting level.

   **v2 correction (the v1 reasoning was wrong)**: v1 claimed
   `Config::max_fiber_frame_depth` covers this "transitively". It
   does not. `max_fiber_frame_depth` bounds Vm frame depth — the
   number of Ruby method/block frames — which is a different
   counter from yield-site recursion. A program with a flat Vm
   frame stack can still deeply yield-recurse: `def f; yield; end`
   called with `f { f { f { f { ... } } } }`. Vm frame depth grows
   linearly here (each `f` call is one frame, each block is one
   frame) but yield-recursion depth ALSO grows linearly — they're
   coincident in this case but conceptually independent. A more
   adversarial case (a method body that builds a chain via
   `define_method` + dynamic dispatch) could grow yield-recursion
   without proportional Vm frame growth.

   **v2 mitigation**: add `Vm::yield_recursion_depth: u32` +
   `Config::max_yield_recursion: Option<u32>` (default Some(256),
   mirroring existing resource-cap conservatism). Cap exhaustion
   traps `ResourceExhausted`.

   **v3 RAII guard (was the round-2 reviewer finding)**: bare
   increment/decrement leaks the counter on panic. Wrap the
   counter mutation in a `YieldDepthGuard<'a>` whose `Drop`
   decrements, same shape as ADR 0013's `VmPtrGuard` and ADR
   0023 v2's `FiberStashGuard`.

   ```rust
   struct YieldDepthGuard<'a> { vm: &'a mut Vm }
   impl<'a> YieldDepthGuard<'a> {
       fn enter(vm: &'a mut Vm) -> Result<Self, Trap> {
           vm.yield_recursion_depth += 1;
           if let Some(cap) = vm.max_yield_recursion
               && vm.yield_recursion_depth > cap {
               vm.yield_recursion_depth -= 1;
               return Err(vm.trap(RubyError::ResourceExhausted {
                   msg: format!(
                       "yield recursion depth exceeded ({} > {})",
                       vm.yield_recursion_depth, cap,
                   ),
               }));
           }
           Ok(Self { vm })
       }
   }
   impl Drop for YieldDepthGuard<'_> {
       fn drop(&mut self) { self.vm.yield_recursion_depth -= 1; }
   }
   ```

   Same shape as `cext_depth` from ADR 0023 — which itself was
   audited as part of this review and confirmed to use a Drop-
   based guard already.

2. **Op::Yield + Fiber yield interaction (resolved in v2).**
   v1 left the IP-advancement question open ("two approaches",
   "heart of the design difficulty"). v2 commits to **option (ii)**:
   a per-Frame `pending_yield: bool` flag.

   Mechanism:
   - Op::Yield SETS `frame.pending_yield = true` BEFORE
     invoke_block. IP is NOT yet advanced.
   - Dispatch enters the block; eventually block does Op::Return
     (or Fiber.yield).
   - On normal return: control returns to dispatch_until inside
     Op::Yield's synchronous wrapper; the wrapper CLEARS
     `pending_yield`, advances IP, runs break-unwind helper if
     `break_signaled`.
   - On Fiber.yield: dispatch_until exits with both
     `fiber_yield_pending` set AND `pending_yield=true` on the
     yielding-method frame. Frame is stashed in FiberSnapshot (the
     `pending_yield` field MUST be added to the v2 FiberSnapshot
     table — see ADR 0023 §"Fiber-scoped Vm state"). The Rust
     synchronous Op::Yield wrapper IS lost when the Rust stack
     unwinds; this is OK.
   - On Fiber resume: frames restored with `pending_yield=true`.
     Dispatch at the yielding-method frame's IP (still at the
     Op::Yield instruction). On op fetch: dispatch sees
     `pending_yield=true`, SKIPS invoke_block (the block frame is
     already on the stack from the original suspend, ready to
     resume), goes straight to the "post-block, check break_signaled,
     advance IP" branch. Clean.

   Key property: `pending_yield` is a per-Frame field. Nested
   concurrent yields each track their own pending state. The
   pending_yield flag is set/cleared by the dispatch_until-internal
   wrapper, not by user bytecode — no Ruby surface.

3. **Coordination with ADR 0025 — `dispatch_until` hot path.** Both
   ADRs modify `dispatch_until`'s top-of-loop. 0024 Phase A adds
   the synchronous Op::Yield wrapper + break-unwind. 0025 Phase 2
   adds `interrupt_pending` check alongside `method_return` /
   `fiber_yield_pending`.

   **v3 corrected merge order** (v2 said "0025 Phase 2 must precede
   0024 Phase A" — that was shorthand; Phase 2 can't literally land
   first without Phase 1 + flag/handler install in place). The
   actual achievable order:

   1. ADR 0025 Phase 0 — Interrupt class hierarchy. **SHIPPED**
      (commit `a5337fd7`).
   2. ADR 0025 Phase 0.5 — SystemExit class. Independent; can
      land any time before Phase 4.
   3. ADR 0025 Phase 1 — `interrupt_pending` flag + Config
      capability + signal-hook handler.
   4. ADR 0025 Phase 2 — safe-point check in `dispatch_until`
      (the actual hot-path edit).
   5. THEN ADR 0024 Phase A — synchronous Op::Yield + break-unwind
      + the cross-ADR interaction handling.

   Alternative: 0024 Phase A's first commit ships before 0025
   Phase 2 AND includes the 0025 Phase 2 work as part of that
   commit. Discouraged: bundles two ADRs' first commits, harder
   to review.

   Interaction case + test: SIGINT arrives during a synchronous
   Op::Yield's nested dispatch_until. Behavior: interrupt_pending
   observed at the inner dispatch_until's top-of-loop, raises
   Interrupt as a Trap, propagates up through the break-unwind
   helper's stack-walk logic (the helper handles non-local exits
   cleanly because it uses the same rescues stack as method_return).
   Test: `def f; yield; end; f { sleep(60) }` + SIGINT → Interrupt
   propagates out of `f`'s frame correctly.

4. **Rust-iter perf regression.** Bytecode iter on a `1_000_000.times`
   benchmark may be noticeably slower than the Rust for-loop (one
   `Op::Yield` + invoke_block + epilogue per iteration vs a tight
   Rust loop with `step_block`). Mitigation paths: (a) keep both,
   prefer Rust when no Fiber in scope; (b) inline-cache the block
   invocation; (c) accept the regression as the cost of correctness.
   Benchmark Phase B before deciding.

5. **Block-break target — CRuby parity (v2 honest restatement).**
   v1 said Phase A pops to "nearest non-block frame." CRuby's
   actual target is the YIELDING method's frame — the method
   whose Op::Yield invoked the block. In the simple case (a method
   `def f; yield; end; f { break }`) "nearest non-block frame"
   IS the yielding method, so they coincide. The cases that
   DIVERGE involve **nested yield chains where the block being
   broken came from a method called as a block from another
   yielding method** — e.g.:

   ```ruby
   def outer; yield; end
   def inner; yield; end
   outer { inner { break } }  # CRuby: break unwinds `inner` (the
                              # yielder of the breaking block)
   ```

   "Nearest non-block frame" gives `inner` in this case too, so
   the divergence is in even more contrived shapes (e.g. blocks
   passed to other blocks). Phase A's "nearest non-block" rule is
   CRuby-correct for the common case + the named extension; the
   adversarial-nesting parity work is **deferred to Phase A round
   2** with an explicit test list to be added once the basic
   mechanism ships. Tracked here so Phase A reviewers don't expect
   100% CRuby parity at first land.

6. **`break val` value semantics.** CRuby's `break val` from a
   block returns val from the yielding method. rubyrs Phase A pins
   this. Documented in the chosen-mechanism section: the break
   value is on the stack from the block's Op::Return; the break-
   unwind helper picks it up and pushes it as the yielding
   method's return value.

7. **Interaction with `ensure`.** A block-break that propagates
   through `def loop; while true; yield; end; end` MUST fire any
   `ensure` in `loop`. CRuby does this. v2 mechanism: the
   break-unwind helper walks the `rescues` stack on the yielding
   method's frame — same code path `method_return` already uses
   for non-local returns. `vm/raise.rs::begin_loop_transfer`
   provides the inner-loop ensure-execution scaffolding; the
   break-unwind helper layers on top with the same shape. Phase A
   test required: `def loop; begin; yield; ensure; @did = true;
   end; end` + `loop { break }` → `@did == true`.

8. **`StopIteration` and `Kernel#loop`.** CRuby `loop` explicitly
   rescues StopIteration and returns the exception's `result`
   attr if present. v1 hand-waved "minimal StopIteration, moot".
   v2 commits: Phase A's `def loop` includes the
   `rescue StopIteration` clause matching CRuby exactly. The
   broader Enumerator / Lazy work (StopIteration-driven external
   iterators) remains out of scope and is gated separately.
   Phase A `def loop` becomes:
   ```ruby
   def loop
     while true; yield; end
   rescue StopIteration => e
     e.respond_to?(:result) ? e.result : nil
   end
   ```

## Test strategy

Phase A:

- `def loop; while true; yield; end; end` + `loop { break }` exits.
- `loop { break val }` returns val.
- Nested `yield` + outermost `break` propagates correctly.
- Existing `step_block`-driven iter (Int#times) — every existing
  iter test must stay green. Verifies no regression.
- Fiber + yield + no break — verifies the synchronous Op::Yield
  doesn't break Fiber suspension semantics.
- Frame-depth limit holds (recursive yield chains hit the cap).

Phase B:

- Promote `p2_21_known_bug_times_loop_inside_fiber_yield` → require
  all chunks.
- All `Int#times` / `upto` / `downto` existing tests stay green.
- All `Array#each` / `Hash#each` / `Range#each` tests stay green.
- Benchmark: 1M `times` iteration regression < 30% slower than
  pre-refactor (target; revisit with real numbers).
- SSE example uses `times` instead of `while`.

## Alternatives considered (summary)

| Alternative | Why rejected |
|---|---|
| Rust-builtin `Kernel#loop` | Inherits Fiber+iter silent-truncation. Doesn't generalize. |
| Mark block frames "Rust-driver-above" | Wiring churn at every invoke_block site; Op::Yield work is structurally cleaner. |
| Bytecode iter only, leave Op::Yield | Doesn't unblock `Kernel#loop` as a Ruby def — `loop`'s `break` still wouldn't propagate. |
| Keep both bugs documented | OK as stopgap (current state); not OK as destination. |

## Revision log

- **2026-05-29 — v3 (this revision).** Second-round review on v2
  surfaced three remaining issues, all closed inline:
  - `yield_recursion_depth` counter now wrapped in
    `YieldDepthGuard<'a>` with Drop-decrement (panic-safe).
    Mirrors ADR 0013's `VmPtrGuard` + ADR 0023's `FiberStashGuard`.
  - Merge-order specification corrected: v2's "0025 Phase 2 must
    precede 0024 Phase A" was shorthand. v3 spells out the actual
    achievable order (0025 Phase 0 done → 0.5 → 1 → 2 → THEN 0024
    Phase A) symmetric with 0025 v3.
  - Phase A step 1 now explicitly notes the ADR 0023
    §"Fiber-scoped Vm state" cross-edit (add `pending_yield`,
    add `yield_recursion_depth` to DO-NOT-stash). ADR 0023 v7
    placeholder rows make the dependency visible from both
    sides.
- **2026-05-29 — v2.** Three parallel reviewer
  rounds (architecture / Rust safety / Ruby parity) on v1 surfaced
  four load-bearing corrections:
  - Risk #1 (Rust-stack bound argument) was WRONG — `max_fiber_frame_depth`
    bounds Vm frame depth, not yield-recursion depth. Added explicit
    `Config::max_yield_recursion` cap + Vm counter.
  - Risk #2 (IP advancement) was an unresolved design hole, not a
    risk. v2 commits to option (ii): per-Frame `pending_yield: bool`
    set/cleared by the Op::Yield wrapper, also stashed in
    FiberSnapshot.
  - Alt 2 (frame-marking) weighting was strawman-ish — now that the
    chosen approach also adds a per-Frame field, Alt 2's cost
    criticism applies in mirror. Three honest reasons to still
    prefer the chosen approach: Kernel#loop unblocking, Phase B
    groundwork, single mechanism for both gaps.
  - Ruby parity gaps named honestly: block-break target spec
    diverges from CRuby in adversarial nesting (Risk #5 deferred
    to Phase A round 2); ensure-on-block-break threaded through
    rescues stack walk (Risk #7 concrete mechanism); StopIteration
    handling in `Kernel#loop` (Risk #8 commits to explicit
    `rescue StopIteration` matching CRuby); Hash#each insertion
    order + Enumerable#map break-with-value pinned in Decision.
  Coordination with ADR 0025 added (Risk #3 new): both touch
  `dispatch_until` hot path; merge order specified.
  Phase A estimate bumped 5-7 → 7-9 commits. Total 15-22 → 17-24
  commits / 4-6 → 5-7 weeks.
- **2026-05-29 — v1.** Initial design after the Kernel#loop
  investigation surfaced the same root cause as the P2 #21 Fiber+iter
  follow-up. Two phases recommended; Phase A unblocks `Kernel#loop`;
  Phase B is the permanent Fiber+iter fix. Risks #2 (yield+Fiber
  recursion / IP advancement) called out as the heart of the design
  difficulty.

## Related

- [ADR 0023](0023-true-async-streaming.md) — Fiber primitive +
  streaming bodies; the P2 #21 follow-up surfaced the silent-
  truncation pattern this ADR proposes to permanently fix.
- [ADR 0017](0017-tier1-boundary.md) — Tier 2 boundary; `Fiber` is
  a Tier 2 feature, so Phase B's bytecode iter drivers must work
  both with and without the `_fiber` feature flag.
- [ADR 0013](0013-cext-vm-aliasing.md) — VmBorrow contract; the
  synchronous Op::Yield + recursive dispatch_until pattern needs to
  honor it (existing `step_block` already does).
