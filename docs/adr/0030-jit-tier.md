# 0030: Closure-threading JIT tier (PoC)

## Status

Proposed / Spike (2026-06). Branch `spike/jit-poc`. This ADR revisits
[0002 — Bytecode VM, not a JIT](0002-bytecode-vm-not-jit.md) along the
exact escape hatch 0002 left open, and reports what a working seam looks
like. It does **not** propose shipping a JIT in `default` or the CLI
bundle.

## Context

ADR 0002 chose a stack-based bytecode VM and said "no JIT for the
foreseeable future", for reasons that still hold: rubyrs targets fast
cold start + small memory (the mruby-competitive space), short programs
run many times, and a JIT's warmup/code-size costs fight all of that.
But 0002 explicitly scoped the door open:

> Reversal cost: low. The `Vm::step()` function is well-isolated. If a
> future contributor wants to plug a JIT in, the boundary is clear (it
> replaces the dispatch loop).

This spike is that contributor. The question it answers is not "should we
ship a JIT" (no — see 0002) but: **what does the seam actually cost, and
what's the cleanest shape for an opt-in, runtime-toggleable JIT that can
never regress correctness?** Two concrete questions framed the work:

1. Should the JIT be a separate crate?
2. Should it be possible to enable/disable at any time?

## Decision (spike findings)

### 1. Separate crate — yes, with a caveat the spike surfaced

Every tier boundary in this workspace is "separate crate + optional dep +
feature flag" (`cext`, `bignum`, `regex`, `carmine`/`rostdown`/
`liquidus`). The JIT follows suit:

- **`crates/rubyrs-jit`** — a standalone crate that depends on *nothing*.
  It owns the backend-agnostic half: the hotness **policy** (`JitConfig`,
  `TierDecision`, `JitConfig::decide`) and the **stats** (`JitStats`).
  This half is unit-testable without standing up a VM, and is the seam a
  future Cranelift backend would also consume.
- **`crates/rubyrs/src/jit.rs`** — the closure-threading **compiler**,
  behind the `jit` feature, *inside* `rubyrs`.

The caveat — and the spike's headline finding — is **why the compiler
can't (yet) live in `rubyrs-jit`**: it must name `Op`, `Proto`, `Vm`,
`Value`, `Locals` and call `Vm::step`, all of which are `pub(crate)`. A
sibling crate cannot see them. So today the split is:

```
rubyrs-jit  (no deps)         rubyrs  (feature "jit")
  JitConfig    ───────────▶     src/jit.rs
  TierDecision   used by          CompiledProto / Thunk
  JitStats                        compile_proto / thunk_for
                                  Vm::{jit_on_invoke, jit_compile, jit_dispatch}
```

Moving the compiler out requires carving a *public, sealed* JIT-facing
surface from `rubyrs` (a `JitHost` trait exposing exactly: read operand
stack / locals, push/pop, charge fuel, run one interpreted op, resolve a
method). That is a real design task (ADR-worthy on its own) and is
deliberately out of scope for the spike. The takeaway: **a separate crate
is the right destination, but the `pub(crate)` wall means the
codegen-that-touches-`Vm` lives in `rubyrs` until that seam is carved.**

### 2. Toggle at any time — yes, on two independent axes

- **Compile-time:** the `jit` cargo feature. Off (the default) → no
  `rubyrs-jit` dep, no `jit` module, and the two dispatch loops cfg back
  to the plain `self.step(op, proto_idx)`. The JIT-less binary is
  byte-identical to today (verified: `cargo check -p rubyrs` clean; the
  one pre-existing debug-stack test flake reproduces identically with and
  without the feature).
- **Runtime:** `Config::jit` (and `RUBYRS_JIT=1` for the CLI), plus a
  tier-up threshold (`RUBYRS_JIT_THRESHOLD`). A build can ship the JIT
  compiled-in but **dormant**; a host flips it on per-`Runtime` with no
  rebuild. This is a **tiered** design: the interpreter is the always-on
  tier-0 baseline; the JIT is tier-1, entered only for protos that cross
  the hotness threshold, and anything it can't model falls back to
  `step`. Enabling the JIT therefore can *never* change observable
  behaviour — only speed.

### Execution model (what the spike built)

Closure-threading. When a proto's invocation count crosses the threshold,
`compile_proto` pre-decodes its `Vec<Op>` into a parallel `Vec<Thunk>`
(`Box<dyn Fn(&mut Vm, usize) -> Result<bool, Trap>>`), one thunk per op,
**same indices** so the existing ip-relative jump offsets work untouched.
The dispatch loops call `jit_dispatch` instead of `step`; for a compiled
proto it runs `thunks[ip]` (the closure *is* the dispatch — no `match`).

Each thunk is either:

- **specialized** — integer arithmetic (`BinOp`/`BinOpInt` Int×Int fast
  path), constant pushes, local reads, stack shuffles — doing the work
  inline; or
- **delegating** — `Box::new(move |vm, pi| vm.step(op, pi))` — handing
  the long tail straight back to the interpreter.

Arithmetic fast paths commit **only** when their result is *provably
identical* to `step` (both operands plain `Int`, no divide-by-zero, no
overflow promotion); every other shape leaves the operand stack pristine
and falls through to `step`. That is what makes the JIT a pure
performance layer.

Two correctness invariants the spike pinned down:

- **Fuel parity.** `step` charges one `check_fuel` per op (the sandbox
  bound, ADR 0008). Specialized thunks bypass `step`, so they charge fuel
  themselves — exactly once per op on whichever path they take. Net: a
  JIT-on run decrements fuel identically to JIT-off.
- **Tier-up chokepoint.** The hot bytecode call path uses inline
  IC-cached frame pushers (`try_invoke_*_cached`) that bypass
  `invoke_method_with_block_inner`, so a per-invoke-site counter misses
  them. The counter instead lives in `jit_dispatch` keyed on `ip == 0` —
  the one point every (re)entered frame passes through. It catches
  methods, blocks, class bodies and `<main>` uniformly.

### Why `Value` makes this the *right* first backend

`Value` is a ~24-byte tagged enum with heap indirection via `ObjId` into
`Vm::heap` — not NaN-boxed, not pointer-tagged. A native (Cranelift)
backend can't keep that in registers; it would have to box/unbox at every
boundary and call back into `Vm` runtime helpers for nearly everything
except raw integer math. Closure-threading sidesteps the representation
problem entirely (it operates on the same `Value` the interpreter does),
which is why it's the correct *spike* backend: it proves the seam,
hotness, toggles, and fuel/stat plumbing without taking on codegen risk.

## Consequences

What the spike demonstrated (branch `spike/jit-poc`, `poc/jit-spike/`):

- **It works end-to-end.** `fib(32)` is byte-identical interpreted vs.
  JIT'd; a differential across arithmetic, bignum, divmod, floats,
  hashes, sorting, strings, recursion, blocks, exceptions, and
  classes-in-hot-loops (threshold = 1, forcing *everything* to tier up)
  is ALL-MATCH.
- **Zero regressions.** The release lib suite is unchanged with the
  feature on (the single failing test fails identically on default
  features — pre-existing, unrelated).
- **Performance is at parity, not a speedup — yet.** `fib(32)`: interp
  ~0.78s vs JIT ~0.77s. Expected for a naive spike: every op pays an
  `Rc<CompiledProto>` clone *per op* in `jit_dispatch`, and ~half of
  fib's ops are delegating thunks (calls, branches) that add a closure
  hop on top of `step`. The wins are bounded by how much is specialized.

The clear path to an actual speedup, in priority order:

1. **Resolve the compiled proto once per *frame*, not per *op*** (drop
   the per-op `Rc` clone). This alone should move the needle.
2. **Specialize the control-flow + local-write ops** (`Jump`,
   `JumpIfFalse`, `StoreLocal`, `IncLocal`, `BinOpLocalLocal`) so a hot
   loop body never touches `step`.
3. **Superinstruction fusion at compile time** (the JIT is the natural
   home for what `BinOpInt`/`BinOpLocalLocal` do by hand today).
4. Only then consider a native backend behind the same seam — which first
   needs the public `JitHost` surface (finding #1).

This ADR does not change ADR 0002's posture for `default`/CLI: no JIT
ships on by default. It records that the seam is real, cheap to keep
dormant, and correctness-preserving by construction — so the decision to
ever turn it on stays a clean, reversible, opt-in one.
