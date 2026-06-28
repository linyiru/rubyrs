# 0032: Native (Cranelift) JIT — surpassing CRuby, and the two ceilings beyond

## Status

Proposed / Spike (2026-06). Opt-in `jit-native` feature; `RUBYRS_JIT_NATIVE=1`
runtime toggle. Continues [0030 — Closure-threading JIT tier](0030-jit-tier.md)
along the exact door 0030 left open ("only then consider a native backend behind
the same seam") and confronts the wall 0030 named but did not cross: the `Value`
representation problem. Does **not** propose shipping a JIT in `default`/CLI
(0002's posture stands).

## Context

0030 built a closure-threading JIT and measured it at *parity, not a speedup* —
every op pays an `Rc` clone and half of `fib`'s ops delegate back to `step`. It
also stated, sharply, why a native backend looked hard (§"Why `Value` makes this
the right first backend"):

> `Value` is a ~24-byte tagged enum with heap indirection… A native (Cranelift)
> backend can't keep that in registers; it would have to box/unbox at every
> boundary and call back into `Vm` runtime helpers for nearly everything except
> raw integer math.

The animating question this spike took on is the one the whole project keeps
circling: **can rubyrs not just match but SURPASS CRuby+YJIT?** 0030 proved the
seam; this ADR asks whether real native codegen, through that seam, can win — and
if so, where it can't, and why.

The work was structured as a layered exploration ("D"): integer codegen → method
calls → control flow → value representation → primitive calls → production
routing → inline-cache dispatch. Each layer was built and measured before the
next.

## Decision (spike findings)

### 1. Native codegen SURPASSES CRuby+YJIT on self-contained hot methods

A real Cranelift backend (`crates/rubyrs-jit` for the smoke-tested primitives,
`crates/rubyrs/src/jit_native.rs` for the `Vm`-coupled lowering) compiles an
eligible `Proto` to machine code. The representation problem 0030 flagged is
sidestepped by **unboxing `Int` locals to raw `i64`** and **deopting on anything
else** (a non-`Int` arg or an arithmetic overflow falls back to the interpreter,
so a compiled method can never change a result — only its speed).

- **Integer methods: ~14.8× YJIT / ~21× CRuby** end-to-end (a polynomial loop
  whose whole body is the method).
- **Recursive `fib(32)`: 2× YJIT, 10.7× CRuby, 52× rubyrs-interp** (15ms vs YJIT
  30ms, CRuby 160ms, interp 786ms). This is the headline: the first time the JIT
  beats YJIT on **call-heavy** code, not just arithmetic — because method CALLS
  compile to native too (a self-recursive call → a Cranelift self-call, with the
  overflow flag threaded through the whole tree).

### 2. The value-representation path works — Values cross by pointer

To compile methods that touch non-integers, the JIT passes `Value`s **by
pointer** and calls rubyrs primitives natively — so the enum's in-memory layout
never enters codegen. Validated end to end:

- A native function calls an external rubyrs primitive through a pointer (the
  seam for `Hash#[]`, `instance_variable_get`, …): proven (`compile_with_helper`).
- A non-integer method returns **every** `Value` type correctly (String, Integer,
  Array, nil, Hash — all match CRuby).
- The AR-shaped `def get(o); o.instance_variable_get(:@v); end` (NOT
  interpreter-fast-pathed) compiles to a single native `jit_ivar_get` and runs
  **3.0× over rubyrs-interp** (1702ms vs 5250ms, `r.get(b)` ×20M).

### 3. Production routing — the JIT must not tax cold code

The spike's first cut globally bypassed the interpreter fast paths when the JIT
was on (~2× slowdown on ALL non-JIT'd code). Replaced with **per-method
selective routing**: the fast path resolves the method, then hands only a method
the JIT can speed up to the compiler; everything else keeps its fast path. Plus
**in-place dispatch** — an already-compiled method runs its native code right in
the fast path, with no slow-path round-trip through `invoke_method_with_block`.
Cold code now pays ~1% (was ~2×); the in-place dispatch alone took `r.get(b)`
from 3707ms → 1702ms.

### 4. Every mechanism of a call-compiling JIT is validated

| Mechanism | Evidence |
|---|---|
| Integer codegen quality | 14.8× YJIT |
| Method-call compilation | `fib` 2× YJIT |
| Control flow with values | block-parameters across BB merges (Layer 2) |
| Value representation | 5 types correct, by-pointer |
| Primitive calls | external-call PoC + `jit_ivar_get` |
| Production routing | cold code +1% (selective) |
| In-place native dispatch | `get` 3707→1702ms |

There is no unsolved *mechanism* between here and an AR-shaped win. What remains
is scope and base speed — finding #5.

### 5. The two ceilings — why call-from-loop does NOT yet surpass CRuby

`fib` surpasses YJIT; `r.get(b)` in a loop wins 3× over rubyrs-interp but stays
**2.7× UNDER CRuby** (1702ms vs 637ms). The difference is decisive and it is
**not codegen**:

- **`fib` is self-contained** — the JIT compiles the *whole* computation (loop +
  recursive calls), so it runs entirely in native code.
- **`get` is a leaf** — the JIT compiles `get`, but the loop driving it
  (`while i<n; s=get(b); i+=1`) stays interpreted. rubyrs's base per-op cost
  (~85ns/iter) is above CRuby's (~32ns/iter), and that loop is the floor.

So the two ceilings to surpassing CRuby on call-heavy AR workloads are:

1. **Compilation scope.** The JIT's reach is exactly what it compiles. Beating
   CRuby on call-from-loop needs the *calling context* compiled too — a method
   JIT over whole call trees, or a trace JIT over hot cross-method loops — not
   more codegen.
2. **Base interpreter speed.** Whatever stays interpreted runs at rubyrs's
   per-op cost, which trails CRuby's. A true call-site inline cache shaves more
   per-call resolution but cannot cross the loop floor alone.

## Consequences

- **The original question is answered with a qualified yes.** rubyrs's native
  codegen *can* surpass CRuby+YJIT — demonstrably, on self-contained hot methods
  (`fib` 2× YJIT). The codegen ceiling is not the wall.
- **For AR (call-from-loop), the wall is scope + base speed, now precisely
  located.** This converts "can we surpass AR?" from an open research question
  into a costed engineering decision: build a call-tree/trace JIT (multi-person-
  month) and/or close the base-interpreter per-op gap.
- **Correctness is preserved by construction.** Deopt on overflow/non-Int;
  value methods return whatever the heap holds; cold code is byte-unchanged;
  `jit-off` and `default` builds are untouched. The differential and the
  `rubyrs-jit` native suite are green.
- **Does not change 0002/0030 posture.** No JIT ships on by default. This ADR
  records that the native seam is real, that it wins where it has full scope,
  and that the remaining walls are named and measured — so the decision to
  invest in compilation scope can be made on data.

### Path forward (priority order, if pursued)

1. **Compile the calling context** — extend from leaf-method JIT to whole call
   trees (or a trace JIT over hot loops). This is the only path that makes
   call-from-loop behave like `fib`.
2. **Call-site inline cache** — cache the resolved native fn at the `Op::Call`
   site, killing the per-call method resolution + cache lookups (shaves per-call
   cost; does not cross the loop floor alone).
3. **Base per-op interpreter speed** — independent of the JIT; lowers the floor
   for everything that stays interpreted.
4. **Carve the `JitHost` surface** (0030 finding #1) so the `Vm`-coupled lowering
   can move out of `rubyrs` into `rubyrs-jit`.

Spike artifacts: `crates/rubyrs-jit/src/native.rs`,
`crates/rubyrs/src/jit_native.rs`, commits `f2b3de4a`..`c0c79af2`.
