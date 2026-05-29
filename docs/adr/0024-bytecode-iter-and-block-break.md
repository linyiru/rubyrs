# 0024: Bytecode-level iter drivers + block-break propagation through `Op::Yield`

## Status

Proposed (2026-05-29). No code change in this ADR — implementation lands
in a follow-up after the design is accepted.

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
   method unwinds: pop frames up to the nearest non-block (yielding
   method) frame, push the break value, clear `break_signaled`.
2. **Rust-level iter drivers become bytecode helpers (Phase B).**
   `Int#times` and friends compile to a small bytecode template
   (`while i < n; yield i; i += 1; end`) rather than a Rust for-loop
   with `step_block`. The bytecode form puts the iteration counter
   in Vm frame locals, which FiberStashGuard already snapshots, so
   Fiber yield naturally resumes mid-iteration.

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
fire-and-forget. But: requires a new field on every BlockFrame plus
a wiring update at every `invoke_block` callsite. Phase A's
synchronous-Op::Yield is structurally cleaner and lays groundwork
for Phase B.

**Alt 3 — Leave the bugs documented forever.** Both already are:
ignored regression test plus preamble doc-block. But the limitation
makes `Kernel#loop` un-installable and locks every Fiber-streaming
body into the `while`-counter idiom forever. Acceptable as a stopgap;
not acceptable as a destination.

## Implementation plan

**Phase A — Op::Yield synchronous + block-break propagation
(~5-7 commits)**:

1. `Op::Yield` synchronous variant: invoke block, dispatch_until to
   block-frame depth, check method_return + break_signaled, no-op
   otherwise. Behavior-preserving for the no-break case — all
   existing yield tests must stay green.
2. Block-break unwind helper: when `break_signaled` is set after
   dispatch_until, walk frames popping block frames + truncating
   stack, then pop the nearest non-block frame, push break value.
   Clear break_signaled.
3. Test: `def loop; while true; yield; end; end` + `loop { break }`
   exits cleanly.
4. Test: `def loop; while true; yield; end; end` + `loop { break val }`
   returns val.
5. Test: nested yield (`def f; xs.each { |x| yield x }; end; f { break }`)
   propagates correctly to f's caller.
6. Test: Rust-iter break (`Int#times`) still works — verifies the
   Op::Yield path doesn't interfere with step_block's existing
   handling.
7. Install `def loop` in `preamble/object.rb` (top-level, not Kernel
   — rubyrs's top-level dispatch walks `toplevel_methods`).
   Reuses Phase A break propagation; verifies the integration.

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

**Total**: ~15-22 commits over 4-6 weeks. Phase A is ~1 week; Phase B
is the bulk.

## Risks + open questions

1. **Op::Yield recursion depth.** Synchronous yield re-enters
   `dispatch_until` recursively from within `step`. Existing
   `step_block` callers use the same pattern, so the Rust stack
   depth grows by ~2-3 frames per recursive `yield`-chain. Deep
   user recursion (Fibonacci-style yield chains) could blow the
   Rust stack. Mitigation: the existing
   `Config::max_fiber_frame_depth` already bounds Vm frame depth;
   Rust stack frames scale with Vm frame depth, so the cap covers
   this transitively.

2. **Op::Yield + Fiber yield interaction.** When the block does
   `Fiber.yield`, `dispatch_until` exits with `fiber_yield_pending`
   set. The synchronous Op::Yield handler's break check sees
   `break_signaled` is false, falls through. The dispatch_until in
   the resume_fiber driver above us reads fiber_yield_pending and
   exits. Resume restores frames including the recursive
   dispatch_until's invocation frame — the Rust stack from before
   the suspend IS gone, but the recursive Op::Yield call site is
   captured in the Vm's IP, so on resume the dispatch continues at
   the IP just past `Op::Yield(argc)`. **Verify:** the Vm frame at
   IP just past Op::Yield, when re-entered, doesn't try to
   re-invoke the block. This needs careful design.

   Potential fix: the synchronous Op::Yield reads the IP BEFORE
   invoke_block and advances IP only AFTER dispatch_until returns
   normally. On Fiber yield, IP stays at Op::Yield — but block frame
   is already on the stack, suspended. On Fiber resume,
   dispatch_until continues; sees block frame on top; runs block
   bytecode until block returns (Op::Return pops). Then back at
   yielding method's IP at Op::Yield. **But IP is the SAME Op::Yield
   that triggered the original recursion** — we'd re-recurse into a
   new dispatch_until. That's wrong.

   This is the heart of the design difficulty. Two approaches:
   (i) advance IP BEFORE invoke_block and tolerate the recursion-on-
   resume edge case, OR (ii) track "yield in progress" state per
   frame so the IP advances only when the yield completes (with or
   without suspension). (ii) is closer to what CRuby does — `yield`
   has a "pending yield" flag on the cfp.

3. **Rust-iter perf regression.** Bytecode iter on a `1_000_000.times`
   benchmark may be noticeably slower than the Rust for-loop (one
   `Op::Yield` + invoke_block + epilogue per iteration vs a tight
   Rust loop with `step_block`). Mitigation paths: (a) keep both,
   prefer Rust when no Fiber in scope; (b) inline-cache the block
   invocation; (c) accept the regression as the cost of correctness.
   Benchmark Phase B before deciding.

4. **`break` value semantics.** CRuby's `break val` from a block
   returns val from the yielding method (and propagates up if the
   yielding method was itself called as a block). rubyrs Phase A
   should pin `break val` returning val; the deeper "break propagation
   through nested yields where the OUTER yielding method's caller is
   itself yielding" is documented as a CRuby parity gap pending
   Phase A round 2.

5. **Interaction with `ensure`.** A block-break that propagates through
   `def loop; while true; yield; end; end` — does it fire any `ensure`
   in `loop`? CRuby: yes, `ensure` always runs on block-break unwind.
   rubyrs's ensure handling lives in
   `vm/raise.rs::begin_loop_transfer`. Phase A needs to thread break
   through the same ensure-execution path that method_return uses.

6. **`StopIteration` and the Enumerator surface.** CRuby's `loop`
   exits cleanly on StopIteration. rubyrs's StopIteration is minimal
   (no Enumerator Lazy / rescue interplay). Out of scope for this
   ADR; tracked separately. Phase A's `def loop` matches CRuby's
   "break + raise StopIteration" exits for the no-Enumerator case.

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
