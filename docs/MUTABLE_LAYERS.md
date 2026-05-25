# Mutable layers in the metaprog runtime

The PoC sequence (PR #8 → #31, ADR 0010) introduced three independent
sources of interior mutability into what used to be a mostly-immutable
value/class model. This doc draws the ownership graph between them so
future changes don't reintroduce the cycle bug we already shipped and
fixed once, and so the borrow-rules picture stays legible.

If you're touching `Class`, `Method`, `Instance`, `MethodClosure`, or
adding a fourth mutable layer, **read this first**.

## The three layers

| Layer | Cell type | Lives on | Mutated by |
|---|---|---|---|
| 1. Class methods | `RefCell<HashMap<SymId, Rc<Method>>>` | `Class.methods` + `Class.singleton_methods` | `def`, `alias_method`, `define_method`, `include`, `extend`, `def self.foo` |
| 2. Closure-method capture | `Rc<RefCell<Vec<Value>>>` shared with the originating block | `MethodClosure.captured` inside a `Method` | Body of any `define_method`-installed method writing to outer-scope locals |
| 3. Eigenclass | `Option<Rc<Class>>` (the Option itself is mutated; the Class inside is layer 1 recursively) | `Instance.singleton_class` | `def obj.foo`, `obj.define_singleton_method`, `Heap::ensure_singleton_class` |

Each layer has a single, narrow mutation entry point. **Don't add
new direct-mutation paths** — route everything through the existing
ops (`Op::DefMethod`, `Op::DefSingletonMethod`,
`Op::DefObjectSingletonMethod`, `Op::DefMethodBlock`,
`Op::DefObjectSingletonMethodBlock`, `Op::AliasMethod`). The GC root
walker depends on this layout being predictable; see
[`heap.rs::Heap::collect`](../crates/rubyrs/src/heap.rs).

## The ownership graph

```
                            ┌──────────────────────────────────────────┐
                            │ Vm                                       │
                            │   classes:  HashMap<SymId, Rc<Class>>   │  ← roots all named classes for the program's life
                            │   toplevel_methods: HashMap<…, Rc<…>>   │
                            └────────────┬─────────────────────────────┘
                                         │
            ┌────────────────────────────▼─────────────────────────────┐
            │ Class                                                    │
            │   name: String                                           │
            │   methods:           RefCell<HashMap<SymId, Rc<Method>>> │  ← layer 1
            │   singleton_methods: RefCell<HashMap<SymId, Rc<Method>>> │  ← layer 1
            │   superclass:        RefCell<Option<Rc<Class>>>          │
            └────────┬─────────────────────────────────────────────────┘
                     │
        Rc<Method>  ─┴────────────────────────────────────────┐
                                                              │
        ┌─────────────────────────────────────────────────────▼──┐
        │ Method                                                 │
        │   defining_class: Option<Weak<Class>>                  │  ← Weak! Cycle break (PR #31 review)
        │   closure:        Option<MethodClosure>                │
        │     └─ captured:  Rc<RefCell<Vec<Value>>>              │  ← layer 2, shared w/ the lexical scope
        └────────────────────────────────────────────────────────┘

  ┌──────────────────────────────────────────────────────────────┐
  │ Instance (heap-managed)                                      │
  │   class:            Rc<Class>           — never re-pointed   │
  │   ivars:            HashMap<SymId, Value>                    │
  │   singleton_class:  Option<Rc<Class>>   — layer 3 (lazy)     │
  │                       └─ same Class shape; superclass = self.class
  └──────────────────────────────────────────────────────────────┘
```

## Why `defining_class` is `Weak<Class>`, not `Rc<Class>`

Originally `Rc<Class>`. PR #31 review caught the bug:

- A singleton method is held by its eigenclass's `methods` table
- The Method's `defining_class` points back at the eigenclass
- The eigenclass is held only by `Instance.singleton_class`
- When the Instance gets swept by the heap GC, `Instance.singleton_class`
  drops its strong ref. But the eigenclass → Method → eigenclass cycle
  is still alive — `Rc`'s strong-count never reaches 0. **Permanent
  leak per object that ever received a singleton method.**

For regular classes the same cycle exists (`Class → Method → Class`) but
is masked: `Vm.classes` holds every named class for the program's
lifetime, so dropping wouldn't matter anyway. Eigenclasses don't have
that anchor.

Fix: `Method.defining_class` is `Option<Weak<Class>>`. `Frame.defining_class`
stays `Option<Rc<Class>>` (upgraded at frame push, kept alive for the
duration of the method invocation). Reading `defining_class` requires
`.upgrade()`; for regular classes the upgrade always succeeds (because
`Vm.classes` keeps the strong ref); for singleton methods the upgrade
succeeds while the method is running (Instance → singleton_class →
method → frame holds the upgraded Rc).

The regression test at
[`embed.rs::singleton_class_closures_do_not_cycle_leak`](../crates/rubyrs/tests/embed.rs)
allocates 1000 short-lived objects with `define_singleton_method`
closures under `max_heap_objects=200`. Under the old Rc-cycle shape it
hits `ResourceExhausted`; under the Weak fix, GC reclaims everything
and the loop completes.

## Borrow-rules picture

The three layers are all `RefCell`-flavoured, so the **only** runtime
hazard is overlapping borrows. The discipline:

- **Hold each `RefCell::borrow()` for the smallest scope possible.**
  Never call back into VM dispatch (`do_call`, `invoke_method_*`,
  `maybe_gc`) while holding any of them. Method tables can be mutated
  by an inner `Op::DefMethod` / `Op::AliasMethod` / etc., and the
  outer borrow would panic on the second borrow_mut.
- **Method-table lookups always clone the `Rc<Method>` out before
  releasing the borrow.** See `lookup_method_uncached` —
  `current.methods.borrow().get(&name_id).cloned()` then drop the
  borrow before chain-walking.
- **`MethodClosure.captured` reads concurrent with the executing
  block.** The block's frame holds the same Rc and reads/writes
  through its `locals: Rc<RefCell<Vec<Value>>>`. The GC marker for
  `HeapObj::Block` walks captured with an `immutable` borrow, drops
  it before recursing — same scoped-borrow rule.
- **`Instance.singleton_class` is `Option<Rc<Class>>` not a `RefCell`**
  on the *option* itself: writes go through `&mut Instance` (already
  borrowed mutably via `Heap::instance_mut`). Reads through
  `&Instance` (via `Heap::instance`). The Class inside the Option
  is shared via Rc and its own methods table is layer 1.

If you write a method that holds `cls.methods.borrow()` and then
calls something that might mutate the same class's methods table
(e.g. ANY recursive VM dispatch), you'll panic. The codebase has
exactly one cross-layer reader (`maybe_gc`'s root walk) and it
takes care to release one borrow before taking the next. Future
additions must do the same — see the comment block in
[`vm/gc.rs::maybe_gc`](../crates/rubyrs/src/vm/gc.rs).

## GC roots through each layer

The mark phase needs to find every `Value::Object` / `Array` / `Hash` /
`Range` / `Block` reachable through any of these layers, or the closure
captures and singleton-method state would get swept:

| From | Walked into |
|---|---|
| `Frame.self_val`, `frame.locals[*]`, `frame.swap_return`, `frame.block_arg` | direct |
| `Vm.stack[*]`, `Vm.pinned[*]` | direct |
| Every `Class` in `Vm.classes`, every Method's `closure.captured` | added in PR #8 (define_method closure-method support) |
| Every Method in `Vm.toplevel_methods` ditto | PR #8 |
| `Instance.singleton_class.methods` closures | added in PR #31 — singleton classes aren't in `Vm.classes`, so the regular walk wouldn't reach them |

If a future change introduces a fourth mutable layer (e.g.
per-Module constants table that holds Values, or anonymous
`Class.new { ... }`), **the GC root walk must add a corresponding
arm**. The check is mechanical: any heap-y Value held only via that
new layer needs an explicit `Heap::visit_value` in
[`heap.rs::Heap::collect`](../crates/rubyrs/src/heap.rs).

## Why this design, not something cleaner

A few options we considered and rejected:

- **Single `Rc<RefCell<Methods>>` per class, shared between regular
  + singleton tables.** Saves one RefCell per class but loses the
  separate dispatch paths in `do_call` (`Value::Class(c)` recv looks
  at `singleton_methods` only; `Value::Object` recv looks at the
  receiver's class chain). Splitting at the data layer is cheaper
  than splitting at the dispatch layer.
- **Arena-allocated Methods, indices instead of `Rc<Method>`.** Would
  eliminate cycles by construction but requires every super lookup
  + frame setup to indirect through the arena. ADR 0003's Rc-plus-GC
  hybrid is already the project's lane; staying consistent matters
  more than the theoretical perf win.
- **Eager-allocate the eigenclass on every Object.** Trade lazy
  allocation cost for predictable layout. Currently `Option<Rc<Class>>`
  is one word when `None`; eager would be `Rc<Class>` always, with
  an empty methods table for objects that never get a singleton
  method. The 1-word saving per Object matters at the
  embedded-Ruby-DSL scale this project targets.

## Cross-references

- [ADR 0010 — Metaprogramming PoC](adr/0010-metaprogramming-poc.md) —
  the umbrella decision that started all of this
- [ADR 0003 — Rc + mark-sweep hybrid GC](adr/0003-rc-plus-mark-sweep-hybrid-gc.md)
  — the Rc/GC split that this builds on
- [ADR 0004 — Block locals share parent Rc](adr/0004-block-locals-share-parent-rc.md)
  — where layer 2's `Rc<RefCell<Vec<Value>>>` first appeared
- PR #8 — `alias_method` / `method_missing` / `define_method` (closure
  capture introduced)
- PR #31 — singleton class + the Method.defining_class Weak cycle break
