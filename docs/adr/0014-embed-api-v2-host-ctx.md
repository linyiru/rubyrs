# 0014: Embed API v2 — `HostCtx` for heap-y arg reads

## Status

Accepted (2026-05). Implements the `HostCtx` handle that ADR
0007's "Consequences" section flagged for "when a real use case
demands it." Three PRs delivered the surface in stages:

- **#37** — introduce `register_fn_v2(name, |ctx, args| ...)`,
  `pub struct HostCtx<'a>`, and the `HostFnSlot::{V1, V2}` enum
  that unifies the storage map. `HostCtx::resolve_array` and
  `HostCtx::resolve_hash` ship in this PR.
- **#40** — first in-tree dogfood: `examples/gemfile/__gemfile_gem_v2`
  consumes `*splat` as Array and `**kwargs` as Hash via the new
  resolvers, retiring the Ruby-side `|`-joining workaround.
- **#43** — close the last Ruby-side translation by adding
  `HostCtx::resolve_sym` (interner widening). After this the
  Gemfile demo's prelude `gem` shim is a one-line forward; the
  unmodified Gemfile reaches the host with native Symbol keys
  and mixed-type values.

Not superseding ADR 0007 — v1 stays the foundation, v2 is a
strict superset. Both live in the same `host_fns` slot.

## Context

ADR 0007 set the v1 host-fn signature:

```rust
F: Fn(&[Value]) -> Result<Value, Trap> + 'static
```

The closure receives evaluated argument `Value`s only. For
primitive shapes (`Int`, `Str`, `Bool`, `Sym`) the closure can
do everything it needs. For heap-y shapes — `Value::Array(id)`,
`Value::Hash(id)` — `id` is an opaque `u32` into a heap the
closure cannot reach. ADR 0007's "Consequences" called this out:

> `HostFn` is `Fn(&[Value]) -> Result<Value, Trap>` with no host
> context. Host code that wants to allocate Arrays/Hashes from
> inside a host fn can't yet. We'll add a `HostCtx` handle when
> a real use case demands it.

The forcing function arrived with the Gemfile demo
(`examples/gemfile/`). A real-shape Rails-style Gemfile uses:

- `*requirements` splat: `gem "rack", ">= 3.0", "< 4.0"`
- `**opts` kwargs: `gem "puma", require: false, platforms: :mri`
- nested block scopes: `group :a, :b do ... end`

The v1 workaround was a Ruby-side prelude that flattened each
heap-y shape to plain Strings before the host fn ran. For the
`gem` line specifically:

```ruby
def gem(name, *requirements, **opts)
  reqs = requirements.join("|")
  require_kw   = opts.key?(:require)   ? opts[:require].to_s   : ""
  platforms_kw = opts.key?(:platforms) ? opts[:platforms].to_s : ""
  __gemfile_gem(name, reqs, require_kw, platforms_kw)
end
```

The pattern works but pushes typing logic into the Ruby side,
where there's no type system to lean on. A regression that
reorders or renames keys lands as silently-empty Strings in the
host, not as a type error. And every embed host that wants to
consume Ruby DSL inputs hits the same gap.

## Decision

Add a v2 surface that runs in parallel with v1, behind a single
shared name slot.

### Public API

```rust
// New struct, public.
pub struct HostCtx<'a> {
    heap: &'a heap::Heap,         // borrowed, read-only
    interner: &'a intern::Interner, // borrowed, read-only
}

impl<'a> HostCtx<'a> {
    pub fn resolve_array(&self, val: &Value) -> Option<&[Value]>;
    pub fn resolve_hash(&self, val: &Value) -> Option<&[(Value, Value)]>;
    pub fn resolve_sym(&self, val: &Value) -> Option<&str>;
}

// New register method, parallel to register_fn.
impl Runtime {
    pub fn register_fn_v2<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&HostCtx, &[Value]) -> Result<Value, Trap> + 'static;
}
```

The three `resolve_*` methods are the entire reading surface.
`Value::Int` / `Value::Bool` / `Value::Str` are already
self-contained (the closure pattern-matches directly).

### Internal storage

A single map holds both versions:

```rust
pub(crate) enum HostFnSlot {
    V1(Rc<dyn Fn(&[Value]) -> Result<Value, Trap>>),
    V2(Rc<dyn Fn(&HostCtx, &[Value]) -> Result<Value, Trap>>),
}

host_fns: HashMap<SymId, HostFnSlot>,
```

A name resolves to one slot. `register_fn` writes V1, `register_fn_v2`
writes V2; same name swaps the slot either direction (locked by
the `register_fn_v2_replaces_prior_v1_registration` and
`register_fn_replaces_prior_v2_registration` embed tests).

### Dispatch

`Vm::invoke_host_fn` matches on the slot:

```rust
match slot {
    HostFnSlot::V1(host) => {
        let vm_ptr: *mut Vm = self;
        with_vm_ptr_set(vm_ptr, || host(args))  // ADR 0013 path
    }
    HostFnSlot::V2(host) => {
        // No CURRENT_VM_PTR plumbing. See "Soundness".
        let ctx = HostCtx::new(&self.heap, &self.interner);
        host(&ctx, args)
    }
}
```

## Soundness — relationship to ADR 0013

ADR 0013 documents the `CURRENT_VM_PTR` thread-local that lets a
re-entrant cext callback obtain a fresh `&mut Vm` while the
outer `do_call` is still parked on `&mut self`. The two
references are time-disjoint, which both Stacked Borrows and
Tree Borrows accept.

V2 deliberately does **not** call `with_vm_ptr_set` on the V2
arm of `invoke_host_fn`. Two reasons, layered:

1. **The V2 closure holds a borrow.** `HostCtx::new(&self.heap, &self.interner)`
   takes two shared references into the VM. If `CURRENT_VM_PTR`
   were re-aimed at `self` for the duration of the V2 call and
   any code reborrowed it as `&mut Vm`, that reborrow would
   alias the live `&self.heap` borrow — heap mutation through
   the inner `&mut` could realloc the backing `Vec<HeapObj>`
   and dangle any slice the closure obtained via `resolve_*`.
   Skipping the ptr means there is no reborrow channel at all.

2. **External v2 closures have no language-level access path.**
   `CURRENT_VM_PTR` is declared `pub(crate)` (a thread-local
   that lives in `vm/cext.rs`). An embed host outside the crate
   has no way to read it. So even if an outer v1/cext frame set
   the TLS to a non-null pointer before entering a v2 call, no
   user code inside the v2 closure can spell the path to it.
   The boundary the design enforces is therefore "unreachable
   from external v2 code," not "TLS is null" — the latter
   wouldn't be true (nested dispatch may have set the ptr from
   an outer frame).

With both layers in place, the slice returned by
`HostCtx::resolve_array` / `resolve_hash` is valid for the entire
closure body without further caveat from outside the crate. The
PR #37 /code-review pass surfaced both layers; PR #43's Copilot
review pass tightened the wording in the docstring from
"static guarantee" (which over-promised) to "unreachable from
external v2 code" (which is precisely what's enforced).

The Miri synthetic test that ADR 0013 added (cext reborrow
pattern) covers the V1 arm. The V2 arm is structurally simpler
— two shared borrows, no raw-pointer demotion — and is covered
by ordinary `cargo test`.

## What's deliberately NOT in v2

Choosing the read-only ctx scope is the design call. We are
**not** widening v2 to include:

- **Heap mutation from inside the closure.** No
  `HostCtx::push_into_array(&mut self, ...)`, no
  `Runtime::alloc_array()` reachable from a v2 fn. Mutation
  needs `&mut Vm`; the only way to get it during a host call is
  through `CURRENT_VM_PTR`, which v2 deliberately doesn't set
  (see Soundness above). cext closures that genuinely need to
  mutate the heap register as v1 and use the existing TLS
  channel.

- **Re-entrant `eval` / `rb_funcall*`.** Same reason. If a host
  fn needs to call back into Ruby, register it as v1 and use
  the cext bridge. The two registration calls are equivalent in
  surface area; "v2 for heap reads, v1 for VM re-entry" is the
  guidance.

- **Class / module / method definition from a v2 closure.**
  These are mutations of `Vm.classes` / `Vm.toplevel_methods`.
  Same constraint, same workaround.

- **Interner mutation.** `HostCtx` borrows `&Interner`, not
  `&mut Interner`. A v2 closure can read existing Symbol names
  but cannot create new ones. Closures that want to intern a
  String into a Symbol have to receive the Symbol already
  interned (build the kwargs Hash with the Symbol key Ruby-side
  and pass it through), not synthesise it host-side.

These omissions are why the design is sound under a narrow set
of borrow rules; widening the API to include any of the above
would force the deferred `UnsafeCell` refactor that ADR 0013
talks about.

## Trade-offs accepted

### Single-slot replacement asymmetry across the API surface

`register_fn` and `register_fn_v2` write into the same
`HostFnSlot` keyed by name. Calling either with a name that's
already registered replaces the prior entry, regardless of
which version wrote it. Both docstrings now spell this out
explicitly.

We chose unified storage over separate `v1_fns` / `v2_fns` maps
because:

- The lookup site is hot. One map = one hash probe.
- Replacement semantics across versions are easier to reason
  about with a single source of truth.
- Versioning the storage doesn't actually buy anything — the
  dispatch site still has to know v1 vs v2 to call the right
  closure shape.

The cost: a v2 closure expecting Array args can be silently
clobbered by a later `register_fn` call under the same name.
Both docstrings call this out. The embed tests pin both
directions.

### Asymmetry with `cext_class_methods` / `cext_instance_methods`

cext-registered singleton / instance methods live in separate
dispatch tables and use `Rc<HostFn>` (v1-only). They don't
benefit from v2 because cext closures genuinely use `&mut Vm`
re-entry (`rb_funcallv`) and would defeat v2's soundness story
anyway. The docstring on `register_fn_v2` calls out that "Class-
or instance-attached methods installed by a C extension live in
independent dispatch tables and are NOT affected by this call."

If a future ADR introduces v2-flavoured class methods, the
two dispatch sites in `vm/dispatch.rs` that consult
`cext_instance_methods` (in the `Value::Object` receiver path)
and `cext_class_methods` (in the `Value::Class` receiver
path) will need parallel slot enums. None of that work is
gated by 0014.

### Interner widening costs nothing today, but pins the API

`HostCtx` now borrows `&Interner` alongside `&Heap`. Both are
existing `Vm` fields; the cost is one extra `&` argument plumbed
through the constructor. The lock-in: any future Interner
refactor that changes `resolve(SymId) -> &str` would also
change the public `HostCtx::resolve_sym` signature. That's
acceptable — `&str` is the natural shape and we're unlikely to
back away from it.

## Why not `&Runtime`?

An earlier sketch (in the Gemfile demo's first README) proposed
`register_fn_v2(&Runtime, &[Value])`. Rejected because:

- `Runtime` wraps `Vm`. Handing the closure `&Runtime` exposes
  every method `Runtime` has — `eval`, `register_fn`,
  `set_stdout`. Most of those mutate state; some can re-enter
  the VM. The soundness story would collapse.
- The closure only needs heap + interner reads. `HostCtx`
  surfaces exactly that, no more.

Narrowing from "the whole Runtime" to "a read-only view"
preserves the soundness argument and pays nothing on the
ergonomics side (the three resolvers are what the consumer
actually wants).

## Why not pass `&Heap` directly?

`HostCtx::new(&heap, &interner)` could in principle be
`pub fn(heap: &Heap, interner: &Interner)`. We keep `HostCtx`
as a wrapper for two reasons:

- **API stability.** The constructor lives behind the `HostCtx`
  type. We can add a fourth borrow (e.g. `&Heap` becomes
  `&Heap + &CallCache`) without breaking the v2 closure
  signature.
- **No leakage of internal types.** `heap::Heap` and
  `intern::Interner` are `pub(crate)`. Exposing them publicly
  would entangle the embed surface with the runtime's
  innards.

## Consequences

### Wins

- The Gemfile demo's prelude is now a one-line forward for
  `gem`. Bundler-style `*splat` + `**kwargs` + Symbol values
  reach the host natively; all typing, validation, and
  per-kwarg branching lives in typed Rust.
- The same shape is reachable to any embedder. `HostCtx`
  resolvers are the standard pattern for Array / Hash / Symbol
  args; no more bespoke Ruby-side flattening per project.
- Dispatch overhead is one `match` (slot enum); the V2 arm has
  no thread-local touch, so it's at least as fast as V1.

### Costs

- Three PRs of API surface to maintain (`HostCtx`,
  `register_fn_v2`, `HostFnSlot`). Replacing v1 entirely would
  be a breaking change; we keep both forever.
- The "v2 cannot mutate" boundary is a teaching cost. Embedders
  who want to register a v2 closure that also calls back into
  Ruby have to know to use v1 instead. The docstring carries
  this guidance; the test suite has no enforced negative case
  for it (you simply can't write the broken pattern without
  reaching into `pub(crate)`).
- Interner widening means `HostCtx` now holds two borrows.
  Future expansions of the type (a fourth resolver borrowing
  e.g. `&CallCache`) compound this.

## Open follow-ups

- **HostCtx for cext closures.** Today cext callbacks are V1
  only. If a future cext spike wants the Array/Hash/Symbol
  ergonomics without the cext re-entry pattern, we can mint a
  `HostCtx` inside `cext_dispatch` — the borrow rules are
  unchanged. Not done because no concrete cext currently asks
  for it.
- **HostCtx::call(&Runtime, name, args).** The biggest gap left
  is "host fn that wants to call another host fn." Today the
  closure can do its own work but cannot reach the Ruby side.
  Closing this requires either re-introducing `CURRENT_VM_PTR`
  on v2 (and losing the soundness story), or the deferred
  `UnsafeCell` refactor from ADR 0013. Both are out of scope
  for 0014.
- **`register_fn_v2` for instance / singleton methods.** Today
  v2 is top-level-only. A `Runtime::register_method_v2(class,
  name, ...)` would land naturally once cext slots gain the
  enum shape.

## Related

- [ADR 0007 — Host embedding API](0007-host-embedding-api.md)
  — the v1 surface, including the "we'll add a `HostCtx`"
  promise that 0014 fulfills.
- [ADR 0013 — `CURRENT_VM_PTR` borrow-aliasing policy](0013-current-vm-ptr-aliasing.md)
  — the soundness machinery v2 routes around by deliberately
  not participating in.
- [PR #37](https://github.com/linyiru/rubyrs/pull/37) — the
  introducing PR (v2 + HostCtx with `resolve_array` /
  `resolve_hash`).
- [PR #40](https://github.com/linyiru/rubyrs/pull/40) — first
  in-tree dogfood (Gemfile demo).
- [PR #43](https://github.com/linyiru/rubyrs/pull/43) — close
  with `resolve_sym` + interner widening.
- [`crates/rubyrs/src/lib.rs`](../../crates/rubyrs/src/lib.rs) —
  `HostCtx` definition + Runtime methods.
- [`crates/rubyrs/src/vm/dispatch.rs`](../../crates/rubyrs/src/vm/dispatch.rs)
  — `Vm::invoke_host_fn` dispatch site; the V2 arm is the
  source of truth for the no-`CURRENT_VM_PTR` decision.
