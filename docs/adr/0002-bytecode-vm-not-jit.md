# 0002: Bytecode VM, not a JIT

## Status

Accepted (2026-05). **Superseded on the JIT question by
[ADR 0034](0034-jit-first-surpass-yjit.md) (2026-06)** — the "no JIT for
the foreseeable future" decision was reversed after PoCs showed a native
Cranelift JIT beats CRuby + YJIT (see ADR [0030](0030-jit-tier.md) /
[0032](0032-jit-native-surpass.md) / [0034](0034-jit-first-surpass-yjit.md)).
The bytecode-VM design itself still stands: it is the always-correct tier-0
interpreter the JIT deopts to.

## Context

We started with a tree-walking interpreter (~5 hours to FizzBuzz, ~9
hours to FizzBuzz + classes). The tree walker was ~10× slower than
CRuby's interpreter and ~15× slower than CRuby + YJIT.

The pull was to add a JIT. CRuby's effort here (YJIT) is excellent —
written in Rust, by Shopify. We could realistically:

1. **JIT path.** Use Cranelift or hand-build a x86-64 / ARM64 emitter.
   Realistic effort: months. Buys peak performance.
2. **Bytecode VM path.** Compile `Expr` to a tagged `Op` enum, dispatch
   in a `match` loop. Realistic effort: hours. Buys ~2× over the
   tree-walker; roughly 3× slower than CRuby's interpreter.
3. **Tree walker forever.** Cheap but caps us very low.

## Decision

Build a **stack-based bytecode VM** (Option 2). No JIT for the
foreseeable future.

The VM dispatches on `Op` via a `match` in `Vm::step()`. Operand stack
+ frame stack + a heap. Specialised `BinOp` op gives a fast path for
Int+Int arithmetic.

## Consequences

Why this is the right call for our niche:

- rubyrs targets **fast cold start + small memory** (mruby competitive
  space). A JIT directly conflicts with both: warmup time eats startup,
  generated code eats memory.
- Edge / CLI / embedded use cases run short programs many times. JIT
  amortisation never happens.
- The market for "fastest Ruby on long-running servers" is taken: CRuby
  + YJIT, TruffleRuby. We shouldn't compete there.
- A bytecode interpreter is small enough to read top to bottom. A JIT
  is not. We want a contributor-friendly project.

What we accept:

- Best-case performance is bounded by interpreter dispatch overhead.
  We will not beat CRuby's interpreter without a JIT, and definitely
  not YJIT.
- Some pure-arithmetic workloads will be visibly slower than CRuby.
  That's OK; that's not where our value is.

Reversal cost: low. The `Vm::step()` function is well-isolated. If a
future contributor wants to plug a JIT in, the boundary is clear (it
replaces the dispatch loop). This ADR doesn't lock us out of that — it
says it's not the priority now.
