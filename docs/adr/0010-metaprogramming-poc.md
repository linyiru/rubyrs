# 0010: Metaprogramming PoC — alias_method, method_missing, define_method

## Status

Accepted (2026-05) as a PoC. The three features stay supported; the
follow-ups listed under "Consequences" are tracked as separate work.

## Context

`docs/SUBSET.md` listed `define_method`, `method_missing`, and friends as
**Explicitly out of scope**. The motivation was simple: a tiny embedded
runtime doesn't need them, and they look architecturally expensive — a
mutable method table, closure capture out of a method body, a fallback
dispatch path. Real Ruby DSLs that we want to host (`Brewfile`,
`Gemfile`, `.gemspec`) do reach for these, so the "we'll never need
them" stance is fragile.

Before committing the project to a Ruby with no metaprogramming, we ran
a PoC to test the assumption that adding them would be expensive. The
PoC also let us measure how much steady-state performance we trade away
for the features.

## Decision

Add three metaprogramming primitives, scoped to the simplest forms that
exercise the hard parts:

1. **`alias_method :new, :old`** — compile-time desugar to a new
   `Op::AliasMethod(new_id, old_id)`. At runtime, copy the existing
   `Rc<Method>` from `class.methods` (or `toplevel_methods`) under the
   new SymId. *Share the Rc.* The alias is intentionally
   indistinguishable from the original at lookup, including
   `defining_class` — so `super` from the aliased name walks the
   same chain.
2. **`method_missing` fallback** — extract a `Vm::try_method_missing`
   helper. Call it at each of the four NoMethodError raise sites
   (`do_call` no_recv + recv, `do_call_block` no_recv + recv) before
   raising. The helper only fires for `Value::Object` receivers; for
   primitives it returns immediately so the original error path runs.
   Prepends the missed name as a Symbol arg.
3. **`define_method(:name) { |args| ... }`** — compile-time
   desugar to `CreateBlock` + new `Op::DefMethodBlock(name_id)`. At
   runtime, pop the BlockHandle, extract its captured Rc, install a
   `Method` whose new `closure: Option<MethodClosure>` field holds the
   shared `Rc<RefCell<Vec<Value>>>`. `invoke_method_with_block` checks
   the closure field first: when present, the frame's `locals` *is* the
   captured Rc — same slots as the lexical scope, writes propagate.
   The arity check uses `n_params` (no default-arg support; the block
   shape doesn't have it).

A single `method_gen` bump per definition / alias invalidates the
per-call-site inline cache. The PoC does not add a new gen counter or a
per-class serial; the existing global gen is conservative enough.

## What we found that changed the picture

1. **The prerequisites were already done.** `Class.methods` was already
   `RefCell<HashMap>` (for class reopening). `method_gen` was already
   the IC invalidation counter. `BlockHandle` already lived on the GC
   heap (P2-13). `defining_class` was already on `Method` (for `super`,
   ADR 0004). The architectural cost we'd budgeted for had been paid
   incrementally by unrelated work.
2. **`define_method` is faster in rubyrs than `def + ivar`.** A
   captured-local lookup is a slot-index into a `Vec<Value>`; an
   `@ivar` is a `HashMap<SymId, Value>` walk. The closure-method is
   the cheaper path. This inverts the usual "metaprogramming costs
   perf" expectation and points at ivars-as-hashmap as the next thing
   to attack.
3. **Steady-state dispatch is ~3× CRuby — but memory is ~5× lighter.**
   See [`examples/metaprog_bench/`](../../crates/rubyrs/examples/metaprog_bench/README.md).
   The 3× gap is the baseline dispatch loop (frame allocation,
   `Rc::clone`-per-call), not anything PoC-introduced. The 5× memory
   win survives unchanged.

## Why these specific designs

### Why share `Rc<Method>` for `alias_method` instead of cloning the inner

Cloning the `Method` struct would force a choice on `defining_class`
that we don't want to make: copy-with-fixup (alias and original
diverge on `super`) or copy-as-is (they don't, but the model gets
confusing). Sharing the Rc means "the alias *is* the same callable" —
no fixup, no divergence, no decision. The cost is one extra
hashmap entry pointing at the same Method.

### Why `Method.closure: Option<MethodClosure>` instead of `Method::{Proto, Block}` enum

An enum is structurally cleaner but ripples through every site that
reads `m.proto_idx` / `m.params`. The optional-field form keeps the
existing call sites unchanged and pushes the dispatch divergence
into one place (`invoke_method_with_block`'s early branch). When
singleton classes land and demand a real method-shape split, an enum
becomes worth it; until then, the field-level form is the smaller
change.

### Why `method_missing` only on `Value::Object`

Per-primitive class chains (Integer's, String's, etc.) aren't fully
populated yet — `primitive_call` dispatches on receiver-type pattern
matching, not on a real class object. Adding `method_missing` on
primitives means routing through their *script-visible* class, which
needs more groundwork than this PoC was scoped for. Object-only is
where 100% of real `method_missing` proxy DSLs live anyway.

### Why GC walks `Vm.classes` method tables now

A `define_method`-installed closure-method holds the captured `Rc`,
and that captured Vec can contain `Value::Object` / `Value::Array` /
etc. Those heap objects are reachable only via the Method, which
lives in `Class.methods` — and `Class.methods` was previously not
walked by the GC (classes are `Rc<Class>` and outlive the heap, so
the GC didn't need to). Adding a flat loop over every class's method
table to gather closure-captured roots fixes this. Cost: one
`if let Some(closure)` check per installed method per GC. Programs
that never call `define_method` short-circuit on the field check.

## Consequences

What gets easier:
- Bundler / Gemfile DSL is now within reach — `gem` calls that route
  through `method_missing` work, and `attr`-like helpers built on
  `define_method` work.
- `Module / include` is the next big metaprogramming item; the
  `Class.methods` mutability assumption and `method_gen`-based IC
  invalidation that we exercised here transfer directly.

What gets harder:
- The Method struct is no longer a pure value. `MethodClosure.captured`
  is interior-mutable. When singleton classes arrive and introduce a
  *third* RefCell layer (per-Instance method table), the borrow-rules
  picture needs a doc — currently each layer is fine on its own.
- GC root walking is no longer O(stack + frames). For programs that
  install many `define_method`s, it's O(stack + frames + total
  installed closure-methods). A counter on `Vm` of how many
  closure-methods exist would let us skip the walk when zero; not
  worth it for the PoC.

Explicitly accepted trade-offs:
- **No `*args` splat in this PoC.** Real `method_missing(name, *args)`
  proxies aren't expressible. Splat is on the existing
  "Not supported (but on roadmap)" list and goes there.
- **Per-iteration dispatch is 3× CRuby's** in microbenchmarks (see
  `examples/metaprog_bench/`). This is the baseline interpreter, not
  the PoC. We choose to ship the PoC and address dispatch separately.
- **No spec coverage from `ruby/spec` yet.** Eight ad-hoc tests in
  `tests/embed.rs` lock the PoC behaviour in; a `ruby/spec` runner
  is its own RFC.

## Follow-ups

Tracked separately so they don't bloat this PR:
- Performance regression CI (`hyperfine` + `/usr/bin/time -l` against
  a baseline) so the 3×-CRuby / 5×-lighter ratios don't drift.
- `ruby/spec` micro-runner: vendored subset of metaprog specs, tagged
  pass/divergence/blocked-by.
- `*args` splat — unblocks `method_missing` as a proxy and
  `define_method` with flexible arity.
- "Mutable layers" doc when singleton classes land.
- `RubyError::is(class_name)` helper so embed tests don't have to
  pattern-match both `RubyError::NoMethodError` and `Uncaught { class_name }`.
