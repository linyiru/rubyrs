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
   `Op::AliasMethod(new_id, old_id)`. At runtime, resolve the source
   `Rc<Method>` via `lookup_method_uncached` (which walks the
   surrounding class's ancestor chain, so inherited methods can be
   aliased) and install the same Rc under the new SymId on the
   *current* class. *Share the Rc.* The alias is intentionally
   indistinguishable from the original at lookup, including
   `defining_class` — so `super` from the aliased name walks the
   *original*'s superclass chain, matching CRuby's "module of
   definition" semantics. A missing source name raises `NameError`.
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
- **`alias_method` / `define_method` outside a class body silently
  install into `toplevel_methods`** instead of raising. The
  compile-time intercept doesn't track class-body context, matching
  the divergence already documented for `attr_*` in
  `compiler.rs:319-326`. Treated as a single shared follow-up: add
  class-body context tracking once, apply to all four intercepts at
  the same time, rather than diverging behaviour across the
  intercept set.

## Follow-ups — final status

All concrete metaprog follow-ups have shipped:

- ✅ **Performance regression CI** — PR #11 (RSS-only first
  cut + per-workload `STATUS` contract) + PR #17 (tightened
  budgets + added wall-time gate with absolute baselines).
  Went through 5+ review rounds reaching a stable shape;
  retrospective in PR #17 body for the design pivot away
  from master-relative gating.
- ✅ **`ruby/spec` micro-runner** — PR #23 introduced
  `crates/rubyrs/spec/` with hand-written DSL (`describe` /
  `it` / `assert_eq` / `assert_raises`) instead of porting
  MSpec. Started with 3 files / 13 examples; grew to
  **6 files / 30 examples** through PRs #28 (class_eval +
  instance_eval) and #31 (singleton method). Tag-file
  mechanism still deferred — current shape is "every example
  must pass"; adopt when we vendor upstream files unmodified.
- ✅ **`*args` splat** in method params — master `a24d7cb`
  shipped the feature; PR #15 widened the GC-root-hole fix
  for the rest-Array allocation window.
- ✅ **`RubyError::is(class_name)` helper** — PR #20.
  Collapses the two-shape pattern-match (direct variant vs
  `Uncaught { class_name }`) every embed test was doing.
- ✅ **`class_eval` / `instance_eval` / `module_eval`** — PR #28.
  Re-uses the existing `is_class_body` machinery (frame flag
  triggers class_stack / visibility_stack pop on return),
  with two documented divergences:
  * `class_eval { 99 }` returns the class, not 99 (because
    of the `is_class_body` Return arm's value semantics);
    locked by `class_eval_spec::returns_the_class_for_now`.
  * `instance_eval { def name; … }` lands on
    `toplevel_methods` (singleton class wasn't in yet).
    See below.
- ✅ **Singleton class — `def obj.foo` +
  `define_singleton_method`** — PR #31. Lazy
  `Instance.singleton_class: Option<Rc<Class>>` whose
  `superclass` is the user-declared class, so dispatch is a
  single chain walk through the existing
  `lookup_method_uncached`. `Object#class` script semantics
  use a separate `Heap::real_class_of` to skip the eigenclass
  (CRuby behaviour). **Surprise caught in review**: storing
  `Rc<Class>` in `Method.defining_class` formed a strong
  cycle (sc → method → defining_class → sc); for regular
  classes this was masked by `Vm.classes` pinning every
  class forever, but eigenclasses leak per-instance. Fixed
  by switching `Method.defining_class` to
  `Option<Weak<Class>>`. See [MUTABLE_LAYERS.md](../MUTABLE_LAYERS.md)
  for the full ownership graph.
- ✅ **"Mutable layers" doc** — [docs/MUTABLE_LAYERS.md](../MUTABLE_LAYERS.md).
  Three layers of interior mutability that metaprog added
  (Class methods tables, MethodClosure.captured, Instance
  eigenclass), with ownership graph, Weak-rationale, borrow
  hazards, and GC-root-walk responsibilities.

Soft items remaining (not blocking):

- **`instance_eval { def name; ... }` auto-routing to the
  receiver's singleton class.** Currently falls through to
  `toplevel_methods` — same shape as the
  `attr_*` / `alias_method` outside-class-body divergence in
  SUBSET.md. Singleton class is in place now (PR #31), so
  this is purely about wiring the class_stack push to use
  the eigenclass during `instance_eval`'s frame entry.
  Estimated ~1 day.
- **Per-primitive `method_missing`** — currently only fires
  for `Value::Object` recv. Int / Str / Sym / Array / Hash
  receivers raise NoMethodError directly. Master has primitive
  class stubs in the preamble now; wiring lookup through
  them is straightforward but each primitive needs its own
  test surface.

## Retrospective — what we learned

Worth recording for the next architectural-PoC sequence:

1. **The prerequisites were paid by unrelated work.** PR #8
   shipped in 250 lines because reopenable classes (PR #N)
   had already moved `Class.methods` to `RefCell<HashMap>`,
   `method_gen` was already the IC invalidation counter,
   block was already heap-resident (P2-13), and
   `defining_class` was already on Method (for `super`,
   ADR 0004). Future ADRs should write a "what does this
   incidentally unlock" coda — three quarters of metaprog's
   landing cost was already paid by features that didn't
   advertise it.

2. **Bench numbers were inversed from intuition.** ADR 0010
   started thinking "metaprog would be slow." Reality
   (`examples/metaprog_bench/`): `define_method` is *faster*
   than `def + @ivar` in rubyrs because captured-locals are
   slot-indexed and ivars hit a HashMap. The bottleneck
   wasn't metaprog at all — it was the dispatch loop. We
   should have run the bench earlier; instead we wrote the
   PoC, then measured, then realised the prior was wrong.

3. **Type-driven discipline catches what comments can't.**
   The Method ↔ eigenclass cycle (caught in PR #31 review)
   wasn't visible at code-review time because everyone
   reading `Rc<Class>` in `Method.defining_class` was right
   for the regular-class case. The cycle only matters for
   eigenclasses, and only matters *after* the Instance is
   collected. The reviewer reasoned through the graph and
   found it; we then switched to `Weak<Class>` so the type
   signature itself enforces the discipline. Lesson:
   architectural changes that move data from "rooted
   forever" to "rooted per-object" need a fresh
   ownership-graph pass, not just a code review.

4. **Review reviewers as carefully as you review code.**
   The 5-round perf-CI review on PR #17, the 4-round
   ruby-spec-runner review on PR #23, and the singleton-
   class cycle review on PR #31 each surfaced material
   improvements. The cost of running a thorough review pass
   per PR was high (review fatigue is real) but the cost of
   shipping each PR without that pass would have been
   higher.

5. **Master moved faster than expected.** Three PRs got
   superseded mid-flight by independent master work:
   `splat in method params` (PR #12 closed; master shipped
   `a24d7cb`), `def self.method` (master `844530f` collided
   on `Op::DefSingletonMethod` name during PR #31 rebase),
   and `**kwargs` (master `98abea1` added a Def field
   between rebases). The "fetch master before opening a PR"
   habit needs to be tighter; for long-running branches,
   rebase weekly or split into smaller PRs.
