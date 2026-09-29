# ADR 0039 — Per-call and per-op fixed cost: a lean plain call/return pair

Date: 2026-09-28
Status: **proposed**. Part of #378 / #385.
- I1 has shipped.
- I2–I5 are planned.
- #438 (the store-forwarding stalls) precedes this ADR.
Follows: ADR 0031 (do_call dispatch core), ADR 0033 (why a wholesale lean rewrite was rejected),
ADR 0037 (tier-2 frame-keeping invariant), ADR 0038 (measure before rewriting).

## Context

Rails-shaped requests still run ~3.3× CRuby (target 1.5×). The work is spread across many
small method calls, so the fixed cost of one call, and of each op around it, matters more
than any single hot path.

### The measurement (starship, x86, verified 2026-09-28)

Workload: `while i < n; o.n; i += 1; end` with `def n; end`. Each iteration runs 8 ops.
The only op that leaves the hot `step()` is `Return`.

Tools:
- `taskset -c 5 perf stat -e cycles:u,instructions:u`.
- A gdb `stepi` tracer (`trace2.py`) that counts every retired instruction of exactly one
  iteration, attributed to the function (or inlined function) it belongs to.

| | instructions / iteration | IPC |
|---|---:|---:|
| CRuby 3.4 | ~328 | ~3.3 |
| rubyrs (master 69845c0d + #438) | 1385 | ~3.5 |

The IPC is similar, so the gap is **instruction count**, not stalls. #438 removed the two
store-forwarding stalls that were the exception: the `Frame` memcpy and the out-of-line
`apply_int`.

Where the 1385 instructions go, grouped from the trace:

| bucket | instructions | what |
|---|---:|---|
| call op | ~550 | `do_call` 82, `try_invoke_explicit_recv_cached` 54, `serve_explicit_recv_method` 75, `lookup_method_cached` 27, `try_class_of` 15, `check_frames` 20, Frame build/write ~40, `Value` push/pop/drop glue, arg moves |
| `Return` (in `step_cold`) | ~260 | `has_ensure` scan, `pending_method_breaks` / `kept_block` / `crossing_walk` tests, `frames.pop`, `cancel_transfers_in_dead_frames`, `dm_share`, `saved_last_match`, `$!` restore test, `stack.truncate`, `class_body` / `swap_return` tests, `release_frame_locals`, `recycle_frame_aux`, and the drop of the popped 192-byte `Frame` |
| per-op overhead × 8 | ~400 | the dispatch loop (~16/op): `control_signals` test, `interrupt_pending` atomic, `frames.last_mut`, `ip += 1`, and a doubly bounds-checked `protos[proto_idx].code[ip]` (`index<Proto>` 27 + `index<Op>` 24 + slices 17 per iteration). `check_fuel` costs 11/op (88 per iteration), and the `step` match ~10/op |
| op bodies | the rest | `LoadLocal`, `BinOpLocalLocal`, `JumpIfFalse`, and so on |

No single item dominates, and the per-op overhead alone is larger than CRuby's whole
iteration.

### What ADR 0033 already ruled out

ADR 0033 measured that inlining hot ops to skip the `step()` call is **net-zero**. It also
concluded that a lean interpreter cannot be retrofitted wholesale into this VM. This ADR
does not reopen either point.

Instead, it targets a narrower thing: the **fixed bookkeeping** that every op and every
call/return pays even when none of the features it serves is in use (fuel, ensure walks,
kept block frames, `$~`, `$!`, class bodies, `swap_return`). CRuby pays for the same
features only on the frames that use them, through frame flags and catch tables.

## Decision (proposed)

Land five independent increments, each as its own PR with its own measurement gate. Order
is by expected payoff over risk. Each estimate comes from the trace above and is **not yet
measured**.

### I1. Plain-return fast path — **shipped** (measured −97 instr/iter, −13% cycles)

- `Op::Return` first tests one conjunction:
  - no pending method break,
  - no pending loop transfer,
  - the frame is not the kept iterator block,
  - the top frame has no aux box (so no ensure, rescue, loop or `$!` state),
  - the frame is not a class body,
  - no `swap_return`,
  - no `dm_share`.
- When the conjunction holds, `Return` only:
  - takes `locals` and `saved_last_match` out of the frame,
  - drops the frame in place with `frames.truncate`, instead of moving 192 bytes out with
    `pop`,
  - restores `$~`,
  - rebalances the operand stack only if the body left residue,
  - releases the locals.

  Every other shape falls through to the unchanged slow path.
- The draft planned a `Frame::plain_return` flag computed at push. It was not needed:
  - The tested fields share the frame's cache lines, so testing them directly is cheap.
  - Having no flag removes the invariant that every site making a frame non-plain must
    clear it. With that invariant gone, the proposed debug assert is not needed either.
- An earlier experiment moved `Return` into the hot `step()` unchanged and gained nothing.
  This result agrees: the cost was the checks and the frame move, not the cold call.
- Measured on starship (5 interleaved rounds, master 701b0de1, verified 2026-09-28):

  | benchmark | before | after |
  |---|---|---|
  | `o.n` loop, instructions | 41.65e9 | 38.74e9 (−97/iter) |
  | `o.n` loop, cycles | 11.7–12.2e9 | 10.18–10.34e9 (−13%) |
  | call0 `o.n` | 82 ns | 70 ns |

### I2. Code pointer cache (est. −50 instr/iter)

- Hold the running frame's `&[Op]` as a raw slice (pointer and length) in `Vm`, refreshed
  wherever the top frame changes. There are 22 push and 9 pop/truncate sites across 9 files,
  including `jit_tier2.rs`, `raise.rs` and `iter.rs`.
- The dispatch loop then does one bounds check, `ip < len`, instead of indexing into
  `protos` and then into `code`.
- Alternative: keep the double index and only elide the `protos` bounds check, using an
  invariant that `proto_idx` is always valid. This is smaller and safer but gains less.
- Risk: a stale cache after a frame change that skipped the refresh. Guard: a
  `debug_assert!` against the slow lookup on every op.
- ADR 0037: tier 2 bails into the interpreter at arbitrary ops, so every tier-2 frame
  push/pop must refresh the cache too. The debug assert covers this under
  `RUBYRS_JIT_TIER2_THRESHOLD=1`.

### I3. One safe-point countdown (est. −60 instr/iter)

- Replace the per-op `check_fuel` and the per-op `interrupt_pending` load with one
  decrementing `safepoint_budget: u32`. On reaching zero it takes a cold path that handles:
  - fuel,
  - the deadline test every 1024 ops,
  - interrupt delivery,
  - tier-2 poll flags.

  The cold path then re-arms the budget.
- Interrupts must stay prompt. SIGINT handling currently reads the atomic every op, so
  either the signal handler also zeroes the budget, or the budget is capped small (for
  example 1024). The second option costs up to 1024 ops of latency; the ADR 0025 v7 safety
  rationale must be re-checked.
- Coupling to preserve:
  - `jit_tier2.rs:2204` reads `op_counter & 1023` for site settling.
  - The http_server battery sets `vm.fuel = Some(n)`.
  - `RUBYRS_FUEL` must trap at exactly the same op count as today, because fixtures and
    embedders observe it.

  So `op_counter` stays as a derived value, or its readers move to the budget.
- This is the riskiest increment and the one with the most observable semantics, so it
  lands third.

### I4. Lean call path and a smaller Frame (est. −100 instr/iter)

- Move rarely used fields out of the 192-byte `Frame` into the existing lazily boxed aux
  (or a second "method extras" box): `lexical_cvar_class`, `saved_last_match`,
  `swap_return`, `kw_given_mask`, and the `pending_yield` family. This cuts the stores at
  push, the drop glue at pop, and the bytes moved.
- Collapse the `do_call → try_invoke_explicit_recv_cached → serve_explicit_recv_method`
  chain for a monomorphic cache hit with simple arity into one inlined path. The work that
  chain repeats today: re-reading the call site, re-deriving the receiver class
  (`try_class_of`), and `check_frames`.
- Each moved field is audited against `gc.rs` root gathering (a field in aux must still be
  rooted) and against the `lint-gc-rooting` rules.

### I5. Fused compare-and-branch for loop headers (est. −50 instr/iter, loops only)

- Add a new op that fuses `BinOpLocalLocal(Lt, a, b)` with the following `JumpIfFalse` (and
  the `Le`, `Gt`, `Ge` variants) when both operands are Int. Anything else falls back to the
  unfused pair.
- This helps `while` loops, not Rails request dispatch, so it lands last and only if the
  earlier increments leave loops as the visible gap.
- The tier-2 and native JITs must learn the new op or treat it as not admitted.

### Gates per increment

- The usual gates: diff_cruby in 4 modes, clippy, both lints, perf/check, embed, and
  `STRESS_GC=1`.
- Evidence of a gain, on starship: `perf stat cycles:u` on the `o.n` loop and on
  `shape-bench.rb` shows a reduction beyond run-to-run noise (≥3 interleaved rounds).
  Instructions retired alone is not proof (see the note in `dispatch.rs`).
- No Rails regression on the Studio.
- An increment that does not show a cycles gain is reverted and recorded here as rejected,
  as ADR 0031 and ADR 0036 did.

## Expected payoff

If every estimate held, the `o.n` iteration would drop from 1385 to ~975 instructions
(−30%), still ~3× CRuby's count.

This ADR does not predict the Rails delta. Rails time is not all per-call fixed cost, and
the share that is has not been measured.

I1 moved the Rails hello median on the Studio from ~1054 to ~1078 req/s (5 interleaved
rounds, 2026-09-28). The load average was ~12 during those runs, so read this as a
direction, not a measurement.

## Consequences

- I1 adds a second `Op::Return` exit. The slow path's checks must stay a superset of the
  fast path's exclusions: any new per-frame state that needs work on return must also be
  added to the fast-path conjunction.
- I2 and I3 add Vm-level caches that duplicate state (the code slice and the budget). Every
  frame change and every fuel or deadline write must keep them in sync. That is 9 files for
  I2 and at least the tier-2 poll, http_server and embed `Config` paths for I3.
- The structural ceiling from ADR 0033 still stands. If these increments land and Rails is
  still far from 1.5×, the remaining lever is the JIT tiers (ADR 0034 and ADR 0037), not
  more interpreter trimming.
